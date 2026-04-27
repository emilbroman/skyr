use std::sync::Arc;

use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use scylla::{
    client::{session::Session, session_builder::SessionBuilder},
    errors::PrepareError,
    statement::prepared::PreparedStatement,
};

use crate::{
    category::Category,
    error::{ConnectError, SdbError},
    incident::{Incident, IncidentId, IncidentReport},
    summary::StatusSummary,
};

/// Maximum number of characters of any one error message kept verbatim when
/// projected into an incident's [`summary`](Incident::summary). Each distinct
/// message is truncated to this length before being deduped and joined; the
/// raw message is stored unmodified in `sdb.incident_reports`.
pub const REPORT_MESSAGE_MAX_CHARS: usize = 512;

/// Joiner placed between distinct messages when computing an incident's
/// [`summary`](Incident::summary).
const SUMMARY_JOINER: &str = "\n\n";

// ---------------------------------------------------------------------------
// Prepared-statement bundles
// ---------------------------------------------------------------------------

macro_rules! prepared_statements {
    ($($struct_name:ident { $($name:ident = $statement:expr,)* })+) => {
        $(
            #[derive(Clone)]
            struct $struct_name {
                $($name: PreparedStatement,)*
            }

            impl $struct_name {
                async fn new(session: &Session) -> Result<Self, PrepareError> {
                    let ($($name,)*) = futures::join!(
                        $(session.prepare($statement)),*
                    );

                    Ok(Self {
                        $($name: $name?,)*
                    })
                }
            }
        )+
    }
}

// Schema version: 3
//
// Replaces the v2 fan-out (incidents_by_id / by_entity / by_org / by_repo /
// by_env, plus a separate open_incidents LWT registry) with a slimmer model:
//
//  - `incidents` is the authoritative store, partitioned by environment QID,
//    clustered by ULID-prefixed incident_id DESC so listings within an
//    environment come back newest-first for free.
//  - `open_incidents_by_entity` doubles as the LWT registry enforcing the
//    at-most-one-open-per-(entity, category) invariant *and* the listing
//    index for `Resource.openIncidents` / `Deployment.openIncidents`.
//
// All TEXT incident IDs are Crockford-base32 ULIDs; lexicographic order
// matches creation-time order. The schema cannot be migrated in place from
// v2 — the keyspace is dropped and recreated when stepping forward.

prepared_statements! {
    TableStatements {
        // Per-entity rollup. Lazily created on first report.
        create_status_summaries_table = r#"
            CREATE TABLE IF NOT EXISTS sdb.status_summaries (
                entity_qid TEXT,
                last_report_at TIMESTAMP,
                last_report_succeeded BOOLEAN,
                open_incident_count INT,
                worst_open_category TEXT,
                consecutive_failure_count INT,
                operational_state TEXT,
                PRIMARY KEY ((entity_qid))
            )
        "#,

        // Authoritative incident store. Partitioned by environment QID so the
        // env-scoped listing is a single partition scan; clustered DESC on
        // the ULID id so the newest incident sits at the front of the
        // partition.
        create_incidents_table = r#"
            CREATE TABLE IF NOT EXISTS sdb.incidents (
                env_qid TEXT,
                incident_id TEXT,
                entity_qid TEXT,
                category TEXT,
                opened_at TIMESTAMP,
                closed_at TIMESTAMP,
                last_report_at TIMESTAMP,
                report_count BIGINT,
                summary TEXT,
                PRIMARY KEY ((env_qid), incident_id)
            ) WITH CLUSTERING ORDER BY (incident_id DESC)
        "#,

        // Slim LWT registry: at most one row per `(entity_qid, category)` for
        // as long as the incident is open. Both enforces the invariant on
        // `open_incident` and serves as the index for
        // `Resource.openIncidents` / `Deployment.openIncidents`.
        create_open_incidents_by_entity_table = r#"
            CREATE TABLE IF NOT EXISTS sdb.open_incidents_by_entity (
                entity_qid TEXT,
                category TEXT,
                incident_id TEXT,
                PRIMARY KEY ((entity_qid), category)
            )
        "#,

        // Append-only stream of failure reports attributed to an incident.
        // Source of truth for the projected `summary` column on the incidents
        // table. Clustered DESC so the most recent report is the first row
        // returned to a paged read; the recompute path reverses in-process
        // when it needs first-seen order.
        create_incident_reports_table = r#"
            CREATE TABLE IF NOT EXISTS sdb.incident_reports (
                incident_id TEXT,
                report_at TIMESTAMP,
                error_message TEXT,
                PRIMARY KEY ((incident_id), report_at)
            ) WITH CLUSTERING ORDER BY (report_at DESC)
        "#,
    }

    PreparedStatements {
        // -- status_summaries ---------------------------------------------

        get_status_summary = r#"
            SELECT last_report_at, last_report_succeeded, open_incident_count,
                   worst_open_category, consecutive_failure_count, operational_state
            FROM sdb.status_summaries
            WHERE entity_qid = ?
        "#,

        upsert_status_summary = r#"
            INSERT INTO sdb.status_summaries (
                entity_qid, last_report_at, last_report_succeeded,
                open_incident_count, worst_open_category,
                consecutive_failure_count, operational_state
            ) VALUES (?, ?, ?, ?, ?, ?, ?)
        "#,

        delete_status_summary = r#"
            DELETE FROM sdb.status_summaries
            WHERE entity_qid = ?
        "#,

        // -- open_incidents_by_entity (LWT) -------------------------------

        claim_open_slot = r#"
            INSERT INTO sdb.open_incidents_by_entity (entity_qid, category, incident_id)
            VALUES (?, ?, ?)
            IF NOT EXISTS
        "#,

        release_open_slot = r#"
            DELETE FROM sdb.open_incidents_by_entity
            WHERE entity_qid = ? AND category = ?
            IF EXISTS
        "#,

        get_open_slot = r#"
            SELECT incident_id
            FROM sdb.open_incidents_by_entity
            WHERE entity_qid = ? AND category = ?
        "#,

        list_open_slots_for_entity = r#"
            SELECT category, incident_id
            FROM sdb.open_incidents_by_entity
            WHERE entity_qid = ?
        "#,

        // -- incidents ----------------------------------------------------

        insert_incident = r#"
            INSERT INTO sdb.incidents (
                env_qid, incident_id, entity_qid, category,
                opened_at, closed_at, last_report_at, report_count, summary
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,

        get_incident = r#"
            SELECT entity_qid, category, opened_at, closed_at,
                   last_report_at, report_count, summary
            FROM sdb.incidents
            WHERE env_qid = ? AND incident_id = ?
        "#,

        list_incidents_in_env = r#"
            SELECT incident_id, entity_qid, category, opened_at, closed_at,
                   last_report_at, report_count, summary
            FROM sdb.incidents
            WHERE env_qid = ?
        "#,

        update_incident_close = r#"
            UPDATE sdb.incidents
            SET closed_at = ?, last_report_at = ?, report_count = ?
            WHERE env_qid = ? AND incident_id = ?
        "#,

        update_incident_append = r#"
            UPDATE sdb.incidents
            SET last_report_at = ?, report_count = ?, summary = ?
            WHERE env_qid = ? AND incident_id = ?
        "#,

        // -- incident_reports ---------------------------------------------

        // LWW on `(incident_id, report_at)` — RQ redeliveries with the same
        // wall-clock timestamp idempotently overwrite a row with itself.
        insert_incident_report = r#"
            INSERT INTO sdb.incident_reports (
                incident_id, report_at, error_message
            ) VALUES (?, ?, ?)
        "#,

        list_incident_reports = r#"
            SELECT report_at, error_message
            FROM sdb.incident_reports
            WHERE incident_id = ?
        "#,
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct ClientBuilder {
    inner: SessionBuilder,
    replication_factor: Option<u32>,
}

impl ClientBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn known_node(mut self, hostname: impl AsRef<str>) -> Self {
        self.inner = self.inner.known_node(hostname);
        self
    }

    /// Sets the replication factor for the `sdb` keyspace. Defaults to 1.
    /// Production deployments should use a higher value for redundancy.
    pub fn replication_factor(mut self, factor: u32) -> Self {
        self.replication_factor = Some(factor);
        self
    }

    pub async fn build(&self) -> Result<Client, ConnectError> {
        let session = Arc::new(self.inner.build().await?);

        let replication_factor = self.replication_factor.unwrap_or(1);
        let create_keyspace_cql = format!(
            "CREATE KEYSPACE IF NOT EXISTS sdb \
             WITH replication = {{'class': 'SimpleStrategy', 'replication_factor': {replication_factor}}}",
        );
        session.query_unpaged(create_keyspace_cql, ()).await?;

        let table_statements = TableStatements::new(&session).await?;

        let (r0, r1, r2, r3) = futures::join!(
            session.execute_unpaged(&table_statements.create_status_summaries_table, ()),
            session.execute_unpaged(&table_statements.create_incidents_table, ()),
            session.execute_unpaged(&table_statements.create_open_incidents_by_entity_table, ()),
            session.execute_unpaged(&table_statements.create_incident_reports_table, ()),
        );
        r0?;
        r1?;
        r2?;
        r3?;

        let statements = PreparedStatements::new(&session).await?;

        Ok(Client {
            session,
            statements: Arc::new(statements),
        })
    }
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// Client for the Status Database.
///
/// SDB is the only access path to the status database. It encapsulates the
/// Scylla schema for incident records and per-entity status summaries, and
/// must not depend on `cdb` or `rdb` — entity QIDs (sourced from `ids`) are
/// the only shared identifier surface.
#[derive(Clone)]
pub struct Client {
    session: Arc<Session>,
    statements: Arc<PreparedStatements>,
}

// ---------------------------------------------------------------------------
// Status summaries
// ---------------------------------------------------------------------------

impl Client {
    /// Returns the per-entity status summary, or `None` if the entity has
    /// never been reported on (or has been terminated).
    pub async fn status_summary(
        &self,
        entity_qid: &str,
    ) -> Result<Option<StatusSummary>, SdbError> {
        let result = self
            .session
            .execute_unpaged(&self.statements.get_status_summary, (entity_qid,))
            .await?;

        let rows = result.into_rows_result()?;

        type Row = (
            DateTime<Utc>,
            bool,
            i32,
            Option<String>,
            i32,
            Option<String>,
        );

        let Some(row) = rows.maybe_first_row::<Row>()? else {
            return Ok(None);
        };

        let (
            last_report_at,
            last_report_succeeded,
            open_incident_count,
            worst_open_category,
            consecutive_failure_count,
            operational_state,
        ) = row;

        let worst_open_category = match worst_open_category {
            None => None,
            Some(s) => Some(s.parse::<Category>()?),
        };

        Ok(Some(StatusSummary {
            entity_qid: entity_qid.to_string(),
            last_report_at,
            last_report_succeeded,
            open_incident_count: open_incident_count.max(0) as u32,
            worst_open_category,
            consecutive_failure_count: consecutive_failure_count.max(0) as u32,
            operational_state,
        }))
    }

    /// Upserts the entity's status summary in full. Lazily creates the row on
    /// first call. The producer/RE is responsible for computing the new
    /// values; SDB does not derive them.
    pub async fn upsert_status_summary(&self, summary: &StatusSummary) -> Result<(), SdbError> {
        let worst_open: Option<&str> = summary.worst_open_category.as_ref().map(|c| c.as_str());
        self.session
            .execute_unpaged(
                &self.statements.upsert_status_summary,
                (
                    summary.entity_qid.as_str(),
                    summary.last_report_at,
                    summary.last_report_succeeded,
                    summary.open_incident_count as i32,
                    worst_open,
                    summary.consecutive_failure_count as i32,
                    summary.operational_state.as_deref(),
                ),
            )
            .await?;
        Ok(())
    }

    /// Deletes the entity's status summary. Called when the producer signals
    /// terminality (deployment reaches DOWN; resource is destroyed). Incident
    /// records are not affected.
    pub async fn delete_status_summary(&self, entity_qid: &str) -> Result<(), SdbError> {
        self.session
            .execute_unpaged(&self.statements.delete_status_summary, (entity_qid,))
            .await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Incident lifecycle
// ---------------------------------------------------------------------------

/// Outcome of attempting to open a new incident.
#[derive(Clone, Debug)]
pub enum OpenIncidentOutcome {
    /// The incident was created. Returns the freshly-written record.
    Opened(Incident),
    /// An incident already exists in the same `(entity, category)` slot.
    /// Returns the existing incident's ID. Callers that wanted to record a
    /// new failure for that slot should call
    /// [`Client::append_failure_to_open_incident`] instead.
    AlreadyOpen { existing_id: IncidentId },
}

/// Outcome of attempting to close an incident.
#[derive(Clone, Debug)]
pub enum CloseIncidentOutcome {
    /// The incident was closed. Returns the updated record.
    Closed(Incident),
    /// No matching open-incident slot existed. Either the incident was
    /// already closed or the `(entity, category)` pair was never open.
    NotOpen,
}

impl Client {
    /// Attempt to open a new incident for `(entity_qid, category)`. Uses LWT
    /// on the `open_incidents_by_entity` registry to ensure at most one open
    /// incident per pair.
    ///
    /// On success, writes the full incident record to `incidents` (partitioned
    /// by `env_qid`) and records the triggering failure as the first row in
    /// `incident_reports`. The cached `summary` column is the (truncated)
    /// `error_message` since this is the only report seen so far.
    ///
    /// `env_qid` is the canonical string form of the environment QID derived
    /// from `entity_qid` by the caller. Both deployment and resource entity
    /// QIDs embed an environment QID; passing it explicitly keeps SDB free of
    /// QID-parsing logic.
    pub async fn open_incident(
        &self,
        entity_qid: &str,
        env_qid: &str,
        category: Category,
        opened_at: DateTime<Utc>,
        error_message: impl Into<String>,
    ) -> Result<OpenIncidentOutcome, SdbError> {
        let error_message = error_message.into();
        let new_id = IncidentId::at(opened_at);
        let new_id_str = new_id.to_string();

        // 1. Try to claim the (entity, category) slot via LWT.
        let claim = self
            .session
            .execute_unpaged(
                &self.statements.claim_open_slot,
                (entity_qid, category.as_str(), new_id_str.as_str()),
            )
            .await?;
        let claim_rows = claim.into_rows_result()?;

        // LWT result rows for `IF NOT EXISTS` always contain `[applied]` as
        // the first column, plus the existing values when not applied.
        type ClaimRow = (
            bool,
            Option<String>, // entity_qid
            Option<String>, // category
            Option<String>, // incident_id (existing)
        );
        let row = claim_rows.first_row::<ClaimRow>()?;
        if !row.0 {
            // Another caller already opened an incident for this slot.
            let existing_id = match row.3 {
                Some(s) => s.parse::<IncidentId>()?,
                None => IncidentId::default(),
            };
            return Ok(OpenIncidentOutcome::AlreadyOpen { existing_id });
        }

        // 2. Record the triggering report.
        self.session
            .execute_unpaged(
                &self.statements.insert_incident_report,
                (new_id_str.as_str(), opened_at, error_message.as_str()),
            )
            .await?;

        // 3. Persist the authoritative incident row. Summary cache is the
        //    (truncated) triggering message at this point — there is exactly
        //    one report.
        let report_count: i64 = 1;
        let summary = truncate_for_summary(&error_message);

        self.session
            .execute_unpaged(
                &self.statements.insert_incident,
                (
                    env_qid,
                    new_id_str.as_str(),
                    entity_qid,
                    category.as_str(),
                    opened_at,
                    None::<DateTime<Utc>>,
                    opened_at,
                    report_count,
                    summary.as_str(),
                ),
            )
            .await?;

        Ok(OpenIncidentOutcome::Opened(Incident {
            id: new_id,
            entity_qid: entity_qid.to_string(),
            category,
            opened_at,
            closed_at: None,
            last_report_at: opened_at,
            report_count: 1,
            summary,
        }))
    }

    /// Append a failure report to an already-open incident. Records the
    /// report verbatim in `incident_reports`, then recomputes and rewrites the
    /// cached `summary` column on the authoritative incident row.
    ///
    /// Returns the updated incident, or `None` if no incident with the given
    /// id exists in the given environment.
    #[allow(clippy::too_many_arguments)]
    pub async fn append_failure_to_open_incident(
        &self,
        incident_id: IncidentId,
        entity_qid: &str,
        env_qid: &str,
        category: Category,
        opened_at: DateTime<Utc>,
        last_report_at: DateTime<Utc>,
        new_report_count: u64,
        error_message: impl Into<String>,
    ) -> Result<Option<Incident>, SdbError> {
        let error_message = error_message.into();
        let report_count_i64 = i64::try_from(new_report_count).unwrap_or(i64::MAX);
        let id_str = incident_id.to_string();

        // 1. Record the failure verbatim in `incident_reports`. LWW on
        //    `(incident_id, report_at)` makes RQ redeliveries idempotent.
        self.session
            .execute_unpaged(
                &self.statements.insert_incident_report,
                (id_str.as_str(), last_report_at, error_message.as_str()),
            )
            .await?;

        // 2. Recompute the cached summary from the canonical report stream.
        let reports = self.list_reports_for_incident(incident_id).await?;
        let summary = compute_summary(&reports);

        // 3. Rewrite the authoritative incident row.
        self.session
            .execute_unpaged(
                &self.statements.update_incident_append,
                (
                    last_report_at,
                    report_count_i64,
                    summary.as_str(),
                    env_qid,
                    id_str.as_str(),
                ),
            )
            .await?;

        // Re-read the canonical row to return a coherent value.
        let mut incident = self.incident_in_env(env_qid, incident_id).await?;
        if let Some(ref mut inc) = incident {
            // Defensive: if read returned different data due to concurrent
            // writes, callers should still see the values they just wrote.
            inc.last_report_at = last_report_at;
            inc.report_count = new_report_count;
            inc.summary = summary;
            inc.category = category;
            inc.opened_at = opened_at;
            inc.entity_qid = entity_qid.to_string();
        }
        Ok(incident)
    }

    /// Close an open incident. Idempotent: a second call with the same
    /// `(entity, category)` will return [`CloseIncidentOutcome::NotOpen`].
    ///
    /// Releases the LWT slot in `open_incidents_by_entity` and stamps
    /// `closed_at` plus the final `last_report_at` / `report_count` on the
    /// authoritative incident row. The cached `summary` is left untouched —
    /// closure does not add a new report row, so its content is the union of
    /// failures seen during the open window.
    pub async fn close_incident(
        &self,
        entity_qid: &str,
        env_qid: &str,
        category: Category,
        closed_at: DateTime<Utc>,
        last_report_at: DateTime<Utc>,
        final_report_count: u64,
    ) -> Result<CloseIncidentOutcome, SdbError> {
        let report_count_i64 = i64::try_from(final_report_count).unwrap_or(i64::MAX);

        // 1. Look up the open slot to learn the incident id.
        let slot = self
            .session
            .execute_unpaged(
                &self.statements.get_open_slot,
                (entity_qid, category.as_str()),
            )
            .await?;
        let slot_rows = slot.into_rows_result()?;
        let Some((id_str,)) = slot_rows.maybe_first_row::<(String,)>()? else {
            return Ok(CloseIncidentOutcome::NotOpen);
        };
        let incident_id = id_str.parse::<IncidentId>()?;

        // 2. Release the LWT slot. We use IF EXISTS to make the close
        //    idempotent; a concurrent close is treated as a success.
        let release = self
            .session
            .execute_unpaged(
                &self.statements.release_open_slot,
                (entity_qid, category.as_str()),
            )
            .await?;
        let release_rows = release.into_rows_result()?;
        // Scylla's LWT response for a `DELETE ... IF EXISTS` carries the
        // `[applied]` flag plus the row's full primary key columns. We do not
        // care about the per-row values (we already have them); we only need
        // to consume the response so the driver does not surface a column-
        // count mismatch when the row shape changes. A concurrent close that
        // observed `applied=false` is benign and is treated as a success.
        type ReleaseRow = (bool, Option<String>, Option<String>, Option<String>);
        let _ = release_rows.maybe_first_row::<ReleaseRow>()?;

        // 3. Stamp the close timestamps on the authoritative row.
        self.session
            .execute_unpaged(
                &self.statements.update_incident_close,
                (
                    closed_at,
                    last_report_at,
                    report_count_i64,
                    env_qid,
                    incident_id.to_string().as_str(),
                ),
            )
            .await?;

        // 4. Re-read for the response.
        let updated = self.incident_in_env(env_qid, incident_id).await?;
        match updated {
            Some(inc) => Ok(CloseIncidentOutcome::Closed(inc)),
            // Should be unreachable in practice, but degrade gracefully.
            None => Ok(CloseIncidentOutcome::NotOpen),
        }
    }

    /// Look up the open incident id (if any) for `(entity_qid, category)`.
    pub async fn find_open_incident_id(
        &self,
        entity_qid: &str,
        category: Category,
    ) -> Result<Option<IncidentId>, SdbError> {
        let result = self
            .session
            .execute_unpaged(
                &self.statements.get_open_slot,
                (entity_qid, category.as_str()),
            )
            .await?;
        let rows = result.into_rows_result()?;
        match rows.maybe_first_row::<(String,)>()? {
            Some((id_str,)) => Ok(Some(id_str.parse::<IncidentId>()?)),
            None => Ok(None),
        }
    }

    /// List the open `(category, incident_id)` pairs for an entity. Used by
    /// the RE to recompute `worst_open_category` and `open_incident_count`
    /// without scanning incident history.
    pub async fn list_open_incidents_for_entity(
        &self,
        entity_qid: &str,
    ) -> Result<Vec<(Category, IncidentId)>, SdbError> {
        let pager = self
            .session
            .execute_iter(
                self.statements.list_open_slots_for_entity.clone(),
                (entity_qid,),
            )
            .await?;
        let mut out = Vec::new();
        let mut stream = pager.rows_stream::<(String, String)>()?;
        while let Some(row) = stream.next().await {
            let (category, id_str) = row?;
            out.push((category.parse::<Category>()?, id_str.parse::<IncidentId>()?));
        }
        Ok(out)
    }

    /// Fetch a single incident by environment + id. Returns `None` if no
    /// such incident exists.
    pub async fn incident_in_env(
        &self,
        env_qid: &str,
        id: IncidentId,
    ) -> Result<Option<Incident>, SdbError> {
        let id_str = id.to_string();
        let result = self
            .session
            .execute_unpaged(&self.statements.get_incident, (env_qid, id_str.as_str()))
            .await?;
        let rows = result.into_rows_result()?;

        type Row = (
            String,                // entity_qid
            String,                // category
            DateTime<Utc>,         // opened_at
            Option<DateTime<Utc>>, // closed_at
            DateTime<Utc>,         // last_report_at
            i64,                   // report_count
            Option<String>,        // summary
        );

        let Some(row) = rows.maybe_first_row::<Row>()? else {
            return Ok(None);
        };

        let (entity_qid, category, opened_at, closed_at, last_report_at, report_count, summary) =
            row;

        Ok(Some(Incident {
            id,
            entity_qid,
            category: category.parse::<Category>()?,
            opened_at,
            closed_at,
            last_report_at,
            report_count: report_count.max(0) as u64,
            summary: summary.unwrap_or_default(),
        }))
    }

    /// List every incident in the given environment, newest-first per the
    /// table's clustering order.
    pub async fn incidents_in_env(&self, env_qid: &str) -> Result<Vec<Incident>, SdbError> {
        let pager = self
            .session
            .execute_iter(self.statements.list_incidents_in_env.clone(), (env_qid,))
            .await?;

        type Row = (
            String,                // incident_id
            String,                // entity_qid
            String,                // category
            DateTime<Utc>,         // opened_at
            Option<DateTime<Utc>>, // closed_at
            DateTime<Utc>,         // last_report_at
            i64,                   // report_count
            Option<String>,        // summary
        );

        let mut out = Vec::new();
        let mut stream = pager.rows_stream::<Row>()?;
        while let Some(row) = stream.next().await {
            let (
                id_str,
                entity_qid,
                category,
                opened_at,
                closed_at,
                last_report_at,
                report_count,
                summary,
            ) = row?;
            out.push(Incident {
                id: id_str.parse::<IncidentId>()?,
                entity_qid,
                category: category.parse::<Category>()?,
                opened_at,
                closed_at,
                last_report_at,
                report_count: report_count.max(0) as u64,
                summary: summary.unwrap_or_default(),
            });
        }
        Ok(out)
    }

    /// List every currently-open incident for an entity, returning the full
    /// records (not just the slot index). Issues one read against
    /// `open_incidents_by_entity` followed by one point read per open slot
    /// against `incidents`. The slot count is bounded by the cardinality of
    /// [`Category`] (currently five) so the per-entity blow-up is fixed.
    pub async fn open_incidents_for_entity(
        &self,
        entity_qid: &str,
        env_qid: &str,
    ) -> Result<Vec<Incident>, SdbError> {
        let slots = self.list_open_incidents_for_entity(entity_qid).await?;
        let mut out = Vec::with_capacity(slots.len());
        for (_category, id) in slots {
            if let Some(inc) = self.incident_in_env(env_qid, id).await? {
                out.push(inc);
            }
        }
        Ok(out)
    }

    /// List every report attributed to `incident_id`, newest-first per the
    /// table's clustering order. Used internally to recompute the cached
    /// summary; exposed publicly so the API/UI can render a per-incident
    /// timeline if it wishes.
    pub async fn list_reports_for_incident(
        &self,
        incident_id: IncidentId,
    ) -> Result<Vec<IncidentReport>, SdbError> {
        let id_str = incident_id.to_string();
        let pager = self
            .session
            .execute_iter(self.statements.list_incident_reports.clone(), (id_str,))
            .await?;
        let mut out = Vec::new();
        let mut stream = pager.rows_stream::<(DateTime<Utc>, String)>()?;
        while let Some(row) = stream.next().await {
            let (report_at, error_message) = row?;
            out.push(IncidentReport {
                report_at,
                error_message,
            });
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Summary projection
// ---------------------------------------------------------------------------

/// Project a slice of [`IncidentReport`]s into the canonical summary string.
///
/// The reports may be supplied in any order; this function reverses if
/// necessary to produce *first-seen* order for distinct messages, truncates
/// each message to [`REPORT_MESSAGE_MAX_CHARS`] chars, drops empties, then
/// joins with [`SUMMARY_JOINER`].
fn compute_summary(reports: &[IncidentReport]) -> String {
    let mut chronological: Vec<&IncidentReport> = reports.iter().collect();
    chronological.sort_by_key(|r| r.report_at);

    let mut seen: Vec<String> = Vec::new();
    for report in chronological {
        let truncated = truncate_for_summary(&report.error_message);
        if truncated.is_empty() {
            continue;
        }
        if !seen.iter().any(|s| s == &truncated) {
            seen.push(truncated);
        }
    }

    seen.join(SUMMARY_JOINER)
}

/// Truncate a single error message to [`REPORT_MESSAGE_MAX_CHARS`] chars,
/// appending an ellipsis if it had to be shortened. Operates on Unicode
/// scalar values so multi-byte input is not split mid-codepoint.
fn truncate_for_summary(message: &str) -> String {
    if message.chars().count() <= REPORT_MESSAGE_MAX_CHARS {
        return message.to_string();
    }
    let mut out = String::with_capacity(REPORT_MESSAGE_MAX_CHARS + 3);
    for (i, ch) in message.chars().enumerate() {
        if i >= REPORT_MESSAGE_MAX_CHARS {
            break;
        }
        out.push(ch);
    }
    out.push_str("...");
    out
}

#[cfg(test)]
mod summary_tests {
    use super::*;

    fn report(secs: i64, msg: &str) -> IncidentReport {
        let report_at = chrono::DateTime::<Utc>::from_timestamp(secs, 0).unwrap();
        IncidentReport {
            report_at,
            error_message: msg.to_string(),
        }
    }

    #[test]
    fn distinct_messages_in_first_seen_order() {
        let reports = [
            report(10, "first"),
            report(20, "second"),
            report(30, "first"),
            report(40, "third"),
        ];
        assert_eq!(compute_summary(&reports), "first\n\nsecond\n\nthird");
    }

    #[test]
    fn order_is_independent_of_input_order() {
        let mut reports = [
            report(40, "third"),
            report(30, "first"),
            report(20, "second"),
            report(10, "first"),
        ];
        assert_eq!(compute_summary(&reports), "first\n\nsecond\n\nthird");
        reports.reverse();
        assert_eq!(compute_summary(&reports), "first\n\nsecond\n\nthird");
    }

    #[test]
    fn empty_messages_are_dropped() {
        let reports = [report(10, ""), report(20, "real"), report(30, "")];
        assert_eq!(compute_summary(&reports), "real");
    }

    #[test]
    fn long_messages_are_truncated_per_segment() {
        let big = "x".repeat(REPORT_MESSAGE_MAX_CHARS + 10);
        let reports = [report(10, &big), report(20, "small")];
        let summary = compute_summary(&reports);
        let segments: Vec<&str> = summary.split(SUMMARY_JOINER).collect();
        assert_eq!(segments.len(), 2);
        assert!(segments[0].ends_with("..."));
        assert_eq!(
            segments[0].chars().count(),
            REPORT_MESSAGE_MAX_CHARS + 3,
            "truncated message keeps the cap plus the ellipsis",
        );
        assert_eq!(segments[1], "small");
    }

    #[test]
    fn empty_input_yields_empty_summary() {
        assert_eq!(compute_summary(&[]), "");
    }

    #[test]
    fn truncate_short_message_is_unchanged() {
        assert_eq!(truncate_for_summary("short"), "short");
    }

    #[test]
    fn truncate_long_message_appends_ellipsis() {
        let s = "y".repeat(REPORT_MESSAGE_MAX_CHARS + 50);
        let out = truncate_for_summary(&s);
        assert!(out.ends_with("..."));
        assert_eq!(out.chars().count(), REPORT_MESSAGE_MAX_CHARS + 3);
    }
}

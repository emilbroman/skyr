use std::sync::Arc;

use chrono::{DateTime, Utc};
use futures_util::{StreamExt, TryStreamExt};
use scylla::{
    client::{session::Session, session_builder::SessionBuilder},
    errors::PrepareError,
    statement::prepared::PreparedStatement,
};
use uuid::Uuid;

use crate::{
    category::Category,
    error::{ConnectError, SdbError},
    incident::{Incident, IncidentId},
    summary::StatusSummary,
};

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

// Schema version: 1
//
// Migration strategy: this crate uses CREATE ... IF NOT EXISTS for all DDL.
// When the schema needs to change, add new CREATE statements (for additive
// changes like new columns or indexes) or implement an explicit migration
// step that checks the current schema version and applies ALTER statements.
// Bump the version comment above when the schema changes.

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

        // Single-incident lookup by id.
        create_incidents_by_id_table = r#"
            CREATE TABLE IF NOT EXISTS sdb.incidents_by_id (
                id UUID,
                entity_qid TEXT,
                category TEXT,
                opened_at TIMESTAMP,
                closed_at TIMESTAMP,
                last_report_at TIMESTAMP,
                report_count BIGINT,
                last_error_message TEXT,
                triggering_report_summary TEXT,
                org_scope TEXT,
                repo_scope TEXT,
                env_scope TEXT,
                PRIMARY KEY ((id))
            )
        "#,

        // Per-entity timeline. Used for `Deployment.incidents` /
        // `Resource.incidents`. Sorted newest first so default listings hit
        // the front.
        create_incidents_by_entity_table = r#"
            CREATE TABLE IF NOT EXISTS sdb.incidents_by_entity (
                entity_qid TEXT,
                opened_at TIMESTAMP,
                id UUID,
                category TEXT,
                closed_at TIMESTAMP,
                last_report_at TIMESTAMP,
                report_count BIGINT,
                last_error_message TEXT,
                triggering_report_summary TEXT,
                PRIMARY KEY ((entity_qid), opened_at, id)
            ) WITH CLUSTERING ORDER BY (opened_at DESC, id ASC)
        "#,

        // Scope tables for `Organization.incidents`,
        // `Repository.incidents`, `Environment.incidents`. The scope key is
        // the canonical string form of the org/repo/env QID respectively.
        create_incidents_by_org_table = r#"
            CREATE TABLE IF NOT EXISTS sdb.incidents_by_org (
                org_scope TEXT,
                opened_at TIMESTAMP,
                id UUID,
                entity_qid TEXT,
                category TEXT,
                closed_at TIMESTAMP,
                last_report_at TIMESTAMP,
                report_count BIGINT,
                last_error_message TEXT,
                triggering_report_summary TEXT,
                PRIMARY KEY ((org_scope), opened_at, id)
            ) WITH CLUSTERING ORDER BY (opened_at DESC, id ASC)
        "#,

        create_incidents_by_repo_table = r#"
            CREATE TABLE IF NOT EXISTS sdb.incidents_by_repo (
                repo_scope TEXT,
                opened_at TIMESTAMP,
                id UUID,
                entity_qid TEXT,
                category TEXT,
                closed_at TIMESTAMP,
                last_report_at TIMESTAMP,
                report_count BIGINT,
                last_error_message TEXT,
                triggering_report_summary TEXT,
                PRIMARY KEY ((repo_scope), opened_at, id)
            ) WITH CLUSTERING ORDER BY (opened_at DESC, id ASC)
        "#,

        create_incidents_by_env_table = r#"
            CREATE TABLE IF NOT EXISTS sdb.incidents_by_env (
                env_scope TEXT,
                opened_at TIMESTAMP,
                id UUID,
                entity_qid TEXT,
                category TEXT,
                closed_at TIMESTAMP,
                last_report_at TIMESTAMP,
                report_count BIGINT,
                last_error_message TEXT,
                triggering_report_summary TEXT,
                PRIMARY KEY ((env_scope), opened_at, id)
            ) WITH CLUSTERING ORDER BY (opened_at DESC, id ASC)
        "#,

        // Open-incident registry. One row per `(entity_qid, category)` for as
        // long as the incident is open. Enforces the at-most-one-open rule via
        // LWT on insert, and is consulted on close to release the slot.
        create_open_incidents_table = r#"
            CREATE TABLE IF NOT EXISTS sdb.open_incidents (
                entity_qid TEXT,
                category TEXT,
                incident_id UUID,
                opened_at TIMESTAMP,
                PRIMARY KEY ((entity_qid), category)
            )
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

        // -- open_incidents (LWT) -----------------------------------------

        claim_open_slot = r#"
            INSERT INTO sdb.open_incidents (entity_qid, category, incident_id, opened_at)
            VALUES (?, ?, ?, ?)
            IF NOT EXISTS
        "#,

        release_open_slot = r#"
            DELETE FROM sdb.open_incidents
            WHERE entity_qid = ? AND category = ?
            IF EXISTS
        "#,

        get_open_slot = r#"
            SELECT incident_id, opened_at
            FROM sdb.open_incidents
            WHERE entity_qid = ? AND category = ?
        "#,

        list_open_slots_for_entity = r#"
            SELECT category, incident_id
            FROM sdb.open_incidents
            WHERE entity_qid = ?
        "#,

        // -- incidents_by_id ----------------------------------------------

        insert_incident_by_id = r#"
            INSERT INTO sdb.incidents_by_id (
                id, entity_qid, category, opened_at, closed_at,
                last_report_at, report_count, last_error_message,
                triggering_report_summary,
                org_scope, repo_scope, env_scope
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,

        get_incident_by_id = r#"
            SELECT entity_qid, category, opened_at, closed_at,
                   last_report_at, report_count, last_error_message,
                   triggering_report_summary, org_scope, repo_scope, env_scope
            FROM sdb.incidents_by_id
            WHERE id = ?
        "#,

        update_incident_by_id_close = r#"
            UPDATE sdb.incidents_by_id
            SET closed_at = ?, last_report_at = ?, report_count = ?,
                last_error_message = ?
            WHERE id = ?
        "#,

        update_incident_by_id_append = r#"
            UPDATE sdb.incidents_by_id
            SET last_report_at = ?, report_count = ?, last_error_message = ?
            WHERE id = ?
        "#,

        // -- incidents_by_entity ------------------------------------------

        insert_incident_by_entity = r#"
            INSERT INTO sdb.incidents_by_entity (
                entity_qid, opened_at, id, category, closed_at,
                last_report_at, report_count, last_error_message,
                triggering_report_summary
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,

        update_incident_by_entity_close = r#"
            UPDATE sdb.incidents_by_entity
            SET closed_at = ?, last_report_at = ?, report_count = ?,
                last_error_message = ?
            WHERE entity_qid = ? AND opened_at = ? AND id = ?
        "#,

        update_incident_by_entity_append = r#"
            UPDATE sdb.incidents_by_entity
            SET last_report_at = ?, report_count = ?, last_error_message = ?
            WHERE entity_qid = ? AND opened_at = ? AND id = ?
        "#,

        list_incidents_by_entity = r#"
            SELECT id, opened_at, category, closed_at, last_report_at,
                   report_count, last_error_message, triggering_report_summary
            FROM sdb.incidents_by_entity
            WHERE entity_qid = ?
        "#,

        // -- incidents_by_org / repo / env --------------------------------

        insert_incident_by_org = r#"
            INSERT INTO sdb.incidents_by_org (
                org_scope, opened_at, id, entity_qid, category, closed_at,
                last_report_at, report_count, last_error_message,
                triggering_report_summary
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,

        update_incident_by_org_close = r#"
            UPDATE sdb.incidents_by_org
            SET closed_at = ?, last_report_at = ?, report_count = ?,
                last_error_message = ?
            WHERE org_scope = ? AND opened_at = ? AND id = ?
        "#,

        update_incident_by_org_append = r#"
            UPDATE sdb.incidents_by_org
            SET last_report_at = ?, report_count = ?, last_error_message = ?
            WHERE org_scope = ? AND opened_at = ? AND id = ?
        "#,

        list_incidents_by_org = r#"
            SELECT id, opened_at, entity_qid, category, closed_at,
                   last_report_at, report_count, last_error_message,
                   triggering_report_summary
            FROM sdb.incidents_by_org
            WHERE org_scope = ?
        "#,

        insert_incident_by_repo = r#"
            INSERT INTO sdb.incidents_by_repo (
                repo_scope, opened_at, id, entity_qid, category, closed_at,
                last_report_at, report_count, last_error_message,
                triggering_report_summary
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,

        update_incident_by_repo_close = r#"
            UPDATE sdb.incidents_by_repo
            SET closed_at = ?, last_report_at = ?, report_count = ?,
                last_error_message = ?
            WHERE repo_scope = ? AND opened_at = ? AND id = ?
        "#,

        update_incident_by_repo_append = r#"
            UPDATE sdb.incidents_by_repo
            SET last_report_at = ?, report_count = ?, last_error_message = ?
            WHERE repo_scope = ? AND opened_at = ? AND id = ?
        "#,

        list_incidents_by_repo = r#"
            SELECT id, opened_at, entity_qid, category, closed_at,
                   last_report_at, report_count, last_error_message,
                   triggering_report_summary
            FROM sdb.incidents_by_repo
            WHERE repo_scope = ?
        "#,

        insert_incident_by_env = r#"
            INSERT INTO sdb.incidents_by_env (
                env_scope, opened_at, id, entity_qid, category, closed_at,
                last_report_at, report_count, last_error_message,
                triggering_report_summary
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,

        update_incident_by_env_close = r#"
            UPDATE sdb.incidents_by_env
            SET closed_at = ?, last_report_at = ?, report_count = ?,
                last_error_message = ?
            WHERE env_scope = ? AND opened_at = ? AND id = ?
        "#,

        update_incident_by_env_append = r#"
            UPDATE sdb.incidents_by_env
            SET last_report_at = ?, report_count = ?, last_error_message = ?
            WHERE env_scope = ? AND opened_at = ? AND id = ?
        "#,

        list_incidents_by_env = r#"
            SELECT id, opened_at, entity_qid, category, closed_at,
                   last_report_at, report_count, last_error_message,
                   triggering_report_summary
            FROM sdb.incidents_by_env
            WHERE env_scope = ?
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

        let (r0, r1, r2, r3, r4, r5, r6) = futures::join!(
            session.execute_unpaged(&table_statements.create_status_summaries_table, ()),
            session.execute_unpaged(&table_statements.create_incidents_by_id_table, ()),
            session.execute_unpaged(&table_statements.create_incidents_by_entity_table, ()),
            session.execute_unpaged(&table_statements.create_incidents_by_org_table, ()),
            session.execute_unpaged(&table_statements.create_incidents_by_repo_table, ()),
            session.execute_unpaged(&table_statements.create_incidents_by_env_table, ()),
            session.execute_unpaged(&table_statements.create_open_incidents_table, ()),
        );
        r0?;
        r1?;
        r2?;
        r3?;
        r4?;
        r5?;
        r6?;

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
    /// on the `open_incidents` registry to ensure at most one open incident
    /// per pair.
    ///
    /// On success, writes the full incident record across the `incidents_by_*`
    /// tables. The provided `org_scope` / `repo_scope` / `env_scope` values
    /// are denormalized scope keys derived from `entity_qid` by the caller —
    /// see [`Client::scope_keys_for`] for a helper.
    #[allow(clippy::too_many_arguments)]
    pub async fn open_incident(
        &self,
        entity_qid: &str,
        category: Category,
        opened_at: DateTime<Utc>,
        last_error_message: impl Into<String>,
        triggering_report_summary: Option<String>,
        org_scope: &str,
        repo_scope: &str,
        env_scope: &str,
    ) -> Result<OpenIncidentOutcome, SdbError> {
        let last_error_message = last_error_message.into();
        let new_id = IncidentId::new();

        // 1. Try to claim the (entity, category) slot via LWT.
        let claim = self
            .session
            .execute_unpaged(
                &self.statements.claim_open_slot,
                (entity_qid, category.as_str(), new_id.as_uuid(), opened_at),
            )
            .await?;
        let claim_rows = claim.into_rows_result()?;

        // LWT result rows for `IF NOT EXISTS` always contain `[applied]` as
        // the first column, plus the existing values when not applied.
        type ClaimRow = (
            bool,
            Option<String>,        // entity_qid
            Option<String>,        // category
            Option<Uuid>,          // incident_id
            Option<DateTime<Utc>>, // opened_at
        );
        let row = claim_rows.first_row::<ClaimRow>()?;
        if !row.0 {
            // Another caller already opened an incident for this slot.
            let existing_id = row.3.map(IncidentId::from_uuid).unwrap_or_default();
            return Ok(OpenIncidentOutcome::AlreadyOpen { existing_id });
        }

        // 2. Persist the incident record across denormalized tables.
        let report_count: i64 = 1;
        let triggering = triggering_report_summary.as_deref();

        let by_id_fut = self.session.execute_unpaged(
            &self.statements.insert_incident_by_id,
            (
                new_id.as_uuid(),
                entity_qid,
                category.as_str(),
                opened_at,
                None::<DateTime<Utc>>,
                opened_at,
                report_count,
                last_error_message.as_str(),
                triggering,
                org_scope,
                repo_scope,
                env_scope,
            ),
        );

        let by_entity_fut = self.session.execute_unpaged(
            &self.statements.insert_incident_by_entity,
            (
                entity_qid,
                opened_at,
                new_id.as_uuid(),
                category.as_str(),
                None::<DateTime<Utc>>,
                opened_at,
                report_count,
                last_error_message.as_str(),
                triggering,
            ),
        );

        let by_org_fut = self.session.execute_unpaged(
            &self.statements.insert_incident_by_org,
            (
                org_scope,
                opened_at,
                new_id.as_uuid(),
                entity_qid,
                category.as_str(),
                None::<DateTime<Utc>>,
                opened_at,
                report_count,
                last_error_message.as_str(),
                triggering,
            ),
        );

        let by_repo_fut = self.session.execute_unpaged(
            &self.statements.insert_incident_by_repo,
            (
                repo_scope,
                opened_at,
                new_id.as_uuid(),
                entity_qid,
                category.as_str(),
                None::<DateTime<Utc>>,
                opened_at,
                report_count,
                last_error_message.as_str(),
                triggering,
            ),
        );

        let by_env_fut = self.session.execute_unpaged(
            &self.statements.insert_incident_by_env,
            (
                env_scope,
                opened_at,
                new_id.as_uuid(),
                entity_qid,
                category.as_str(),
                None::<DateTime<Utc>>,
                opened_at,
                report_count,
                last_error_message.as_str(),
                triggering,
            ),
        );

        let (r0, r1, r2, r3, r4) = futures::join!(
            by_id_fut,
            by_entity_fut,
            by_org_fut,
            by_repo_fut,
            by_env_fut
        );
        r0?;
        r1?;
        r2?;
        r3?;
        r4?;

        Ok(OpenIncidentOutcome::Opened(Incident {
            id: new_id,
            entity_qid: entity_qid.to_string(),
            category,
            opened_at,
            closed_at: None,
            last_report_at: opened_at,
            report_count: 1,
            last_error_message,
            triggering_report_summary,
        }))
    }

    /// Append a failure report to an already-open incident, bumping
    /// `report_count`, `last_report_at`, and `last_error_message`.
    ///
    /// `opened_at` is required because the per-entity / per-scope tables key
    /// on it; callers typically obtain it from the previously-loaded incident
    /// record (or via [`Client::find_open_incident_id`]). Returns the updated
    /// incident, or `None` if no incident with the given id exists.
    #[allow(clippy::too_many_arguments)]
    pub async fn append_failure_to_open_incident(
        &self,
        incident_id: IncidentId,
        entity_qid: &str,
        category: Category,
        opened_at: DateTime<Utc>,
        last_report_at: DateTime<Utc>,
        new_report_count: u64,
        last_error_message: impl Into<String>,
        org_scope: &str,
        repo_scope: &str,
        env_scope: &str,
    ) -> Result<Option<Incident>, SdbError> {
        let last_error_message = last_error_message.into();
        let report_count_i64 = i64::try_from(new_report_count).unwrap_or(i64::MAX);

        let by_id_fut = self.session.execute_unpaged(
            &self.statements.update_incident_by_id_append,
            (
                last_report_at,
                report_count_i64,
                last_error_message.as_str(),
                incident_id.as_uuid(),
            ),
        );

        let by_entity_fut = self.session.execute_unpaged(
            &self.statements.update_incident_by_entity_append,
            (
                last_report_at,
                report_count_i64,
                last_error_message.as_str(),
                entity_qid,
                opened_at,
                incident_id.as_uuid(),
            ),
        );

        let by_org_fut = self.session.execute_unpaged(
            &self.statements.update_incident_by_org_append,
            (
                last_report_at,
                report_count_i64,
                last_error_message.as_str(),
                org_scope,
                opened_at,
                incident_id.as_uuid(),
            ),
        );

        let by_repo_fut = self.session.execute_unpaged(
            &self.statements.update_incident_by_repo_append,
            (
                last_report_at,
                report_count_i64,
                last_error_message.as_str(),
                repo_scope,
                opened_at,
                incident_id.as_uuid(),
            ),
        );

        let by_env_fut = self.session.execute_unpaged(
            &self.statements.update_incident_by_env_append,
            (
                last_report_at,
                report_count_i64,
                last_error_message.as_str(),
                env_scope,
                opened_at,
                incident_id.as_uuid(),
            ),
        );

        let (r0, r1, r2, r3, r4) = futures::join!(
            by_id_fut,
            by_entity_fut,
            by_org_fut,
            by_repo_fut,
            by_env_fut
        );
        r0?;
        r1?;
        r2?;
        r3?;
        r4?;

        // Re-read the canonical row to return a coherent value.
        let mut incident = self.get_incident(incident_id).await?;
        if let Some(ref mut inc) = incident {
            // Defensive: if read returned different data due to concurrent
            // writes, callers should still see the values they just wrote.
            inc.last_report_at = last_report_at;
            inc.report_count = new_report_count;
            inc.last_error_message = last_error_message;
            // category and opened_at must match the caller-provided values
            // since LWT ensures these are stable; assert by overwriting in
            // case of corrupt rows.
            inc.category = category;
            inc.opened_at = opened_at;
            inc.entity_qid = entity_qid.to_string();
        }
        Ok(incident)
    }

    /// Close an open incident. Idempotent: a second call with the same
    /// `(entity, category)` will return [`CloseIncidentOutcome::NotOpen`].
    ///
    /// Releases the LWT slot in `open_incidents` and stamps `closed_at` /
    /// updates the trailing fields across all incident tables.
    #[allow(clippy::too_many_arguments)]
    pub async fn close_incident(
        &self,
        entity_qid: &str,
        category: Category,
        closed_at: DateTime<Utc>,
        last_report_at: DateTime<Utc>,
        final_report_count: u64,
        last_error_message: impl Into<String>,
        org_scope: &str,
        repo_scope: &str,
        env_scope: &str,
    ) -> Result<CloseIncidentOutcome, SdbError> {
        let last_error_message = last_error_message.into();
        let report_count_i64 = i64::try_from(final_report_count).unwrap_or(i64::MAX);

        // 1. Look up the open slot to learn the incident id and opened_at
        //    timestamps needed to address the by-entity/by-scope rows.
        let slot = self
            .session
            .execute_unpaged(
                &self.statements.get_open_slot,
                (entity_qid, category.as_str()),
            )
            .await?;
        let slot_rows = slot.into_rows_result()?;
        let Some((incident_uuid, opened_at)) =
            slot_rows.maybe_first_row::<(Uuid, DateTime<Utc>)>()?
        else {
            return Ok(CloseIncidentOutcome::NotOpen);
        };
        let incident_id = IncidentId::from_uuid(incident_uuid);

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
        // The DELETE ... IF EXISTS row contains [applied] and the previous
        // values when applied. We do not require it to be applied, since
        // a concurrent close with the same intent is benign.
        let _ = release_rows.maybe_first_row::<(bool,)>()?;

        // 3. Update all denormalized rows.
        let by_id_fut = self.session.execute_unpaged(
            &self.statements.update_incident_by_id_close,
            (
                closed_at,
                last_report_at,
                report_count_i64,
                last_error_message.as_str(),
                incident_id.as_uuid(),
            ),
        );

        let by_entity_fut = self.session.execute_unpaged(
            &self.statements.update_incident_by_entity_close,
            (
                closed_at,
                last_report_at,
                report_count_i64,
                last_error_message.as_str(),
                entity_qid,
                opened_at,
                incident_id.as_uuid(),
            ),
        );

        let by_org_fut = self.session.execute_unpaged(
            &self.statements.update_incident_by_org_close,
            (
                closed_at,
                last_report_at,
                report_count_i64,
                last_error_message.as_str(),
                org_scope,
                opened_at,
                incident_id.as_uuid(),
            ),
        );

        let by_repo_fut = self.session.execute_unpaged(
            &self.statements.update_incident_by_repo_close,
            (
                closed_at,
                last_report_at,
                report_count_i64,
                last_error_message.as_str(),
                repo_scope,
                opened_at,
                incident_id.as_uuid(),
            ),
        );

        let by_env_fut = self.session.execute_unpaged(
            &self.statements.update_incident_by_env_close,
            (
                closed_at,
                last_report_at,
                report_count_i64,
                last_error_message.as_str(),
                env_scope,
                opened_at,
                incident_id.as_uuid(),
            ),
        );

        let (r0, r1, r2, r3, r4) = futures::join!(
            by_id_fut,
            by_entity_fut,
            by_org_fut,
            by_repo_fut,
            by_env_fut
        );
        r0?;
        r1?;
        r2?;
        r3?;
        r4?;

        // 4. Re-read for the response.
        let updated = self.get_incident(incident_id).await?;
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
    ) -> Result<Option<(IncidentId, DateTime<Utc>)>, SdbError> {
        let result = self
            .session
            .execute_unpaged(
                &self.statements.get_open_slot,
                (entity_qid, category.as_str()),
            )
            .await?;
        let rows = result.into_rows_result()?;
        match rows.maybe_first_row::<(Uuid, DateTime<Utc>)>()? {
            Some((id, opened_at)) => Ok(Some((IncidentId::from_uuid(id), opened_at))),
            None => Ok(None),
        }
    }

    /// List the open `(category, incident_id)` pairs for an entity. Used by
    /// callers (notably the RE) to recompute `worst_open_category` and the
    /// `open_incident_count` summary fields without scanning incident
    /// history.
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
        let mut stream = pager.rows_stream::<(String, Uuid)>()?;
        while let Some(row) = stream.next().await {
            let (category, id) = row?;
            out.push((category.parse::<Category>()?, IncidentId::from_uuid(id)));
        }
        Ok(out)
    }

    /// Fetch a single incident by id. Returns `None` if no such incident
    /// exists.
    pub async fn get_incident(&self, id: IncidentId) -> Result<Option<Incident>, SdbError> {
        let result = self
            .session
            .execute_unpaged(&self.statements.get_incident_by_id, (id.as_uuid(),))
            .await?;
        let rows = result.into_rows_result()?;

        type Row = (
            String,                // entity_qid
            String,                // category
            DateTime<Utc>,         // opened_at
            Option<DateTime<Utc>>, // closed_at
            DateTime<Utc>,         // last_report_at
            i64,                   // report_count
            String,                // last_error_message
            Option<String>,        // triggering_report_summary
            String,                // org_scope
            String,                // repo_scope
            String,                // env_scope
        );

        let Some(row) = rows.maybe_first_row::<Row>()? else {
            return Ok(None);
        };

        let (
            entity_qid,
            category,
            opened_at,
            closed_at,
            last_report_at,
            report_count,
            last_error_message,
            triggering_report_summary,
            _org_scope,
            _repo_scope,
            _env_scope,
        ) = row;

        Ok(Some(Incident {
            id,
            entity_qid,
            category: category.parse::<Category>()?,
            opened_at,
            closed_at,
            last_report_at,
            report_count: report_count.max(0) as u64,
            last_error_message,
            triggering_report_summary,
        }))
    }
}

// ---------------------------------------------------------------------------
// Listing
// ---------------------------------------------------------------------------

/// Filter predicate for listing incidents. All fields are optional and
/// combine with logical AND. Pagination is handled by the caller via
/// `offset` and `limit`.
#[derive(Clone, Debug, Default)]
pub struct IncidentFilter {
    /// If `Some`, only include incidents in the given category.
    pub category: Option<Category>,
    /// If `true`, only include currently-open incidents (`closed_at IS NULL`).
    pub open_only: bool,
    /// If `Some`, only include incidents whose `opened_at >= since`.
    pub since: Option<DateTime<Utc>>,
    /// If `Some`, only include incidents whose `opened_at < until`.
    pub until: Option<DateTime<Utc>>,
}

/// Pagination parameters for incident listings.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pagination {
    /// Number of leading rows to skip after applying the filter.
    pub offset: usize,
    /// Maximum number of rows to return after `offset`. `None` means
    /// unlimited.
    pub limit: Option<usize>,
}

impl Client {
    /// List incidents whose entity QID matches `entity_qid`, newest first.
    pub async fn incidents_by_entity(
        &self,
        entity_qid: &str,
        filter: &IncidentFilter,
        pagination: Pagination,
    ) -> Result<Vec<Incident>, SdbError> {
        let pager = self
            .session
            .execute_iter(
                self.statements.list_incidents_by_entity.clone(),
                (entity_qid,),
            )
            .await?;
        let entity_qid = entity_qid.to_string();

        type Row = (
            Uuid,                  // id
            DateTime<Utc>,         // opened_at
            String,                // category
            Option<DateTime<Utc>>, // closed_at
            DateTime<Utc>,         // last_report_at
            i64,                   // report_count
            String,                // last_error_message
            Option<String>,        // triggering_report_summary
        );

        let stream = pager.rows_stream::<Row>()?.map(move |row| {
            let (
                id,
                opened_at,
                category,
                closed_at,
                last_report_at,
                report_count,
                last_error_message,
                triggering_report_summary,
            ) = row?;
            Ok::<_, SdbError>(Incident {
                id: IncidentId::from_uuid(id),
                entity_qid: entity_qid.clone(),
                category: category.parse::<Category>()?,
                opened_at,
                closed_at,
                last_report_at,
                report_count: report_count.max(0) as u64,
                last_error_message,
                triggering_report_summary,
            })
        });

        collect_filtered(stream, filter, pagination).await
    }

    /// List incidents within an organization scope.
    pub async fn incidents_by_org(
        &self,
        org_scope: &str,
        filter: &IncidentFilter,
        pagination: Pagination,
    ) -> Result<Vec<Incident>, SdbError> {
        self.incidents_by_scope(
            self.statements.list_incidents_by_org.clone(),
            org_scope,
            filter,
            pagination,
        )
        .await
    }

    /// List incidents within a repository scope.
    pub async fn incidents_by_repo(
        &self,
        repo_scope: &str,
        filter: &IncidentFilter,
        pagination: Pagination,
    ) -> Result<Vec<Incident>, SdbError> {
        self.incidents_by_scope(
            self.statements.list_incidents_by_repo.clone(),
            repo_scope,
            filter,
            pagination,
        )
        .await
    }

    /// List incidents within an environment scope.
    pub async fn incidents_by_env(
        &self,
        env_scope: &str,
        filter: &IncidentFilter,
        pagination: Pagination,
    ) -> Result<Vec<Incident>, SdbError> {
        self.incidents_by_scope(
            self.statements.list_incidents_by_env.clone(),
            env_scope,
            filter,
            pagination,
        )
        .await
    }

    async fn incidents_by_scope(
        &self,
        statement: PreparedStatement,
        scope: &str,
        filter: &IncidentFilter,
        pagination: Pagination,
    ) -> Result<Vec<Incident>, SdbError> {
        let pager = self.session.execute_iter(statement, (scope,)).await?;

        type Row = (
            Uuid,                  // id
            DateTime<Utc>,         // opened_at
            String,                // entity_qid
            String,                // category
            Option<DateTime<Utc>>, // closed_at
            DateTime<Utc>,         // last_report_at
            i64,                   // report_count
            String,                // last_error_message
            Option<String>,        // triggering_report_summary
        );

        let stream = pager.rows_stream::<Row>()?.map(|row| {
            let (
                id,
                opened_at,
                entity_qid,
                category,
                closed_at,
                last_report_at,
                report_count,
                last_error_message,
                triggering_report_summary,
            ) = row?;
            Ok::<_, SdbError>(Incident {
                id: IncidentId::from_uuid(id),
                entity_qid,
                category: category.parse::<Category>()?,
                opened_at,
                closed_at,
                last_report_at,
                report_count: report_count.max(0) as u64,
                last_error_message,
                triggering_report_summary,
            })
        });

        collect_filtered(stream, filter, pagination).await
    }
}

async fn collect_filtered<S>(
    stream: S,
    filter: &IncidentFilter,
    pagination: Pagination,
) -> Result<Vec<Incident>, SdbError>
where
    S: futures::Stream<Item = Result<Incident, SdbError>>,
{
    let raw: Vec<Incident> = stream.try_collect().await?;
    let mut out = Vec::new();
    let mut skipped = 0usize;
    for incident in raw {
        if let Some(c) = filter.category
            && incident.category != c
        {
            continue;
        }
        if filter.open_only && incident.closed_at.is_some() {
            continue;
        }
        if let Some(since) = filter.since
            && incident.opened_at < since
        {
            continue;
        }
        if let Some(until) = filter.until
            && incident.opened_at >= until
        {
            continue;
        }
        if skipped < pagination.offset {
            skipped += 1;
            continue;
        }
        out.push(incident);
        if let Some(limit) = pagination.limit
            && out.len() >= limit
        {
            break;
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Scope helpers
// ---------------------------------------------------------------------------

/// Denormalized scope keys derived from an entity QID. The SDB stores these
/// alongside each incident row to support `Organization.incidents`,
/// `Repository.incidents`, and `Environment.incidents` queries without a
/// secondary index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeKeys {
    pub org_scope: String,
    pub repo_scope: String,
    pub env_scope: String,
}

impl Client {
    /// Compute the org/repo/env scope keys for a [`DeploymentQid`] or
    /// [`ResourceQid`]. This is the canonical helper for callers writing
    /// incidents — see [`scope_keys_for_deployment`] and
    /// [`scope_keys_for_resource`] for the typed variants.
    pub fn scope_keys_for(qid: EntityRef<'_>) -> ScopeKeys {
        match qid {
            EntityRef::Deployment(d) => ScopeKeys {
                org_scope: d.environment.repo.org.to_string(),
                repo_scope: d.environment.repo.to_string(),
                env_scope: d.environment.to_string(),
            },
            EntityRef::Resource(r) => ScopeKeys {
                org_scope: r.environment.repo.org.to_string(),
                repo_scope: r.environment.repo.to_string(),
                env_scope: r.environment.to_string(),
            },
        }
    }
}

/// Borrowed view of an entity QID. Used to compute denormalized scope keys
/// without coupling SDB to specific entity-type wrappers.
#[derive(Clone, Copy, Debug)]
pub enum EntityRef<'a> {
    Deployment(&'a ids::DeploymentQid),
    Resource(&'a ids::ResourceQid),
}

/// Convenience: compute scope keys for a deployment QID.
pub fn scope_keys_for_deployment(qid: &ids::DeploymentQid) -> ScopeKeys {
    Client::scope_keys_for(EntityRef::Deployment(qid))
}

/// Convenience: compute scope keys for a resource QID.
pub fn scope_keys_for_resource(qid: &ids::ResourceQid) -> ScopeKeys {
    Client::scope_keys_for(EntityRef::Resource(qid))
}

//! Per-report processing pipeline.
//!
//! The pipeline operates on the report's common base — the entity QID,
//! timestamp, outcome, and metrics. Entity-aware decisions (cadence
//! expectations, scope key derivation) go through the small [`crate::entity`]
//! module so the abstraction does not leak into the pipeline body.
//!
//! ## Idempotency at the pipeline level
//!
//! The RQ delivers reports at-least-once. The pipeline must therefore be safe
//! to re-execute on the same report:
//!
//! - **Open path.** [`sdb::Client::open_incident`] uses an LWT on the
//!   `(entity, category)` slot in `open_incidents`. A duplicate first failure
//!   that would have opened an incident now sees the slot occupied and falls
//!   back to bumping counters via `append_failure_to_open_incident`. No
//!   duplicate incident is created.
//! - **NQ Opened emission.** Emitted only on the LWT-applied path of an
//!   `open_incident` call. The redelivered failure that lost the LWT race
//!   does not emit an `Opened` notification. The [`nq::NotificationRequest`]
//!   carries an idempotency key of `(incident_id, OPENED)`, so any spurious
//!   double emission is also de-duped at the NE.
//! - **Close path.** [`sdb::Client::close_incident`] is idempotent: a second
//!   call for an already-closed `(entity, category)` returns `NotOpen`. NQ
//!   emission is gated on the `Closed` outcome, so duplicate success reports
//!   do not produce duplicate `Closed` notifications.
//! - **Status summary upserts** are last-writer-wins. A redelivered report
//!   may overwrite the row with the same values, which is benign.
//! - **Terminal flag.** Deleting a row that has already been deleted is also
//!   benign for ScyllaDB.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use rq::{IncidentCategory, Outcome, Report};
use sdb::{Category, CloseIncidentOutcome, OpenIncidentOutcome, StatusSummary};
use thiserror::Error;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::entity::{operational_state_str, scope_keys};
use crate::thresholds::{ThresholdConfig, ThresholdTracker};

/// Errors arising from the per-report pipeline. All of these warrant a `nack`
/// without requeue (i.e., let the broker DLX/drop the message rather than
/// hot-loop the same poison-pill).
#[derive(Debug, Error)]
pub enum PipelineError {
    #[error("sdb error: {0}")]
    Sdb(#[from] sdb::SdbError),

    #[error("nq publish error: {0}")]
    Nq(#[from] nq::PublishError),
}

/// Shared, cheaply-cloneable handle bundling the worker's external
/// dependencies and its in-memory state.
#[derive(Clone)]
pub struct PipelineContext {
    pub sdb: sdb::Client,
    pub nq: nq::Publisher,
    pub thresholds: Arc<ThresholdConfig>,
    pub tracker: Arc<Mutex<ThresholdTracker>>,
    /// Last-seen cache used by the watchdog. Keyed on entity QID string.
    /// Updated on every report; rows for terminated entities are removed.
    pub heartbeats: Arc<Mutex<HeartbeatCache>>,
}

/// Worker-local cache of `(last_report_at, operational_state)` per entity,
/// used by the watchdog to avoid scanning SDB.
#[derive(Default)]
pub struct HeartbeatCache {
    inner: std::collections::HashMap<String, HeartbeatEntry>,
}

#[derive(Clone, Debug)]
pub struct HeartbeatEntry {
    pub last_report_at: DateTime<Utc>,
    pub operational_state: Option<String>,
    /// Whether the most recent report flagged this resource as volatile.
    /// Non-volatile resources do not receive periodic Check messages while
    /// they sit in `Live`, so the watchdog must not expect heartbeats from
    /// them. Always `false` for deployment entries.
    pub volatile: bool,
    /// Suppresses repeated watchdog firing while an incident is already open
    /// for this entity. Cleared whenever a real heartbeat arrives.
    pub watchdog_open: bool,
}

impl HeartbeatCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert(&mut self, entity_qid: &str, entry: HeartbeatEntry) {
        self.inner.insert(entity_qid.to_string(), entry);
    }

    pub fn remove(&mut self, entity_qid: &str) {
        self.inner.remove(entity_qid);
    }

    pub fn snapshot(&self) -> Vec<(String, HeartbeatEntry)> {
        self.inner
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    pub fn mark_watchdog_open(&mut self, entity_qid: &str) {
        if let Some(entry) = self.inner.get_mut(entity_qid) {
            entry.watchdog_open = true;
        }
    }
}

/// Process one report end-to-end. Returns when SDB and NQ have both been
/// updated; the caller then acks the RQ delivery.
pub async fn process_report(ctx: &PipelineContext, report: &Report) -> Result<(), PipelineError> {
    let entity_qid_string = report.entity_qid.as_string();
    let now = Utc::now();
    let scopes = scope_keys(&report.entity_qid);
    let op_state = operational_state_str(&report.extension);

    // Load current summary so we can compute deltas (consecutive failure
    // counter primarily).
    let prev = ctx.sdb.status_summary(&entity_qid_string).await?;

    let consecutive_failure_count = match (&report.outcome, &prev) {
        (Outcome::Success, _) => 0,
        (Outcome::Failure { .. }, Some(s)) => s.consecutive_failure_count.saturating_add(1),
        (Outcome::Failure { .. }, None) => 1,
    };

    // ---- Apply incident logic for failure / success ---------------------

    match &report.outcome {
        Outcome::Failure {
            category,
            error_message,
        } => {
            handle_failure(
                ctx,
                &entity_qid_string,
                *category,
                error_message,
                report.timestamp,
                now,
                &scopes,
            )
            .await?;
        }
        Outcome::Success => {
            handle_success(ctx, &entity_qid_string, report.timestamp, &scopes).await?;
            // Reset any pre-open accounting; producer is reporting success.
            ctx.tracker.lock().await.forget_entity(&entity_qid_string);
        }
    }

    // ---- Recompute the summary ------------------------------------------

    let open_list = ctx
        .sdb
        .list_open_incidents_for_entity(&entity_qid_string)
        .await?;
    let open_incident_count = open_list.len() as u32;
    let worst_open_category = open_list.iter().map(|(c, _)| *c).max();

    let summary = StatusSummary {
        entity_qid: entity_qid_string.clone(),
        last_report_at: report.timestamp,
        last_report_succeeded: report.outcome.is_success(),
        open_incident_count,
        worst_open_category,
        consecutive_failure_count,
        operational_state: Some(op_state.to_string()),
    };
    ctx.sdb.upsert_status_summary(&summary).await?;

    // ---- Maintain the watchdog cache ------------------------------------

    {
        let mut hb = ctx.heartbeats.lock().await;
        if report.extension.is_terminal() {
            hb.remove(&entity_qid_string);
        } else {
            hb.upsert(
                &entity_qid_string,
                HeartbeatEntry {
                    last_report_at: report.timestamp,
                    operational_state: Some(op_state.to_string()),
                    volatile: report.extension.is_volatile(),
                    // A real report clears any prior watchdog suppression so
                    // the next missed heartbeat can fire fresh.
                    watchdog_open: false,
                },
            );
        }
    }

    // ---- Terminal flag: delete the summary row --------------------------

    if report.extension.is_terminal() {
        ctx.sdb.delete_status_summary(&entity_qid_string).await?;
        ctx.tracker.lock().await.forget_entity(&entity_qid_string);
    }

    Ok(())
}

async fn handle_failure(
    ctx: &PipelineContext,
    entity_qid: &str,
    category: IncidentCategory,
    error_message: &str,
    report_at: DateTime<Utc>,
    now: DateTime<Utc>,
    scopes: &sdb::ScopeKeys,
) -> Result<(), PipelineError> {
    // 1. Already open? Append.
    if let Some((id, opened_at)) = ctx.sdb.find_open_incident_id(entity_qid, category).await? {
        debug!(
            entity_qid = %entity_qid,
            category = %category,
            incident_id = %id,
            "appending failure to existing open incident"
        );
        // We do not have a stored count for the next bump value; the SDB
        // record holds it. Fetch fresh to bump precisely.
        let existing = ctx.sdb.get_incident(id).await?;
        let next_count = existing.map(|inc| inc.report_count + 1).unwrap_or(1);
        ctx.sdb
            .append_failure_to_open_incident(
                id,
                entity_qid,
                category,
                opened_at,
                report_at,
                next_count,
                error_message,
                &scopes.org_scope,
                &scopes.repo_scope,
                &scopes.env_scope,
            )
            .await?;
        return Ok(());
    }

    // 2. Not yet open: consult the threshold tracker.
    let rule = ctx.thresholds.rule_for(category);
    let tripped = ctx
        .tracker
        .lock()
        .await
        .record_and_check(entity_qid, category, rule, report_at, now);

    if !tripped {
        debug!(
            entity_qid = %entity_qid,
            category = %category,
            "below threshold; accumulating failure"
        );
        return Ok(());
    }

    // 3. Threshold tripped: try to open.
    let outcome = ctx
        .sdb
        .open_incident(
            entity_qid,
            category,
            report_at,
            error_message,
            &scopes.org_scope,
            &scopes.repo_scope,
            &scopes.env_scope,
        )
        .await?;

    match outcome {
        OpenIncidentOutcome::Opened(incident) => {
            info!(
                entity_qid = %entity_qid,
                category = %category,
                incident_id = %incident.id,
                "opened incident"
            );
            let request = nq::NotificationRequest {
                incident_id: incident.id.to_string(),
                event_type: nq::NotificationEventType::Opened,
                entity_qid: entity_qid.to_string(),
                category: nq_category(category),
                opened_at: incident.opened_at,
                closed_at: None,
                summary: Some(incident.summary.clone()),
            };
            ctx.nq.enqueue(&request).await?;
        }
        OpenIncidentOutcome::AlreadyOpen { existing_id } => {
            // Another worker (or a concurrent retry) won the LWT race. Treat
            // this report as a bump on the existing incident.
            let existing = ctx.sdb.get_incident(existing_id).await?;
            if let Some(inc) = existing {
                ctx.sdb
                    .append_failure_to_open_incident(
                        inc.id,
                        entity_qid,
                        category,
                        inc.opened_at,
                        report_at,
                        inc.report_count + 1,
                        error_message,
                        &scopes.org_scope,
                        &scopes.repo_scope,
                        &scopes.env_scope,
                    )
                    .await?;
            } else {
                warn!(
                    entity_qid = %entity_qid,
                    category = %category,
                    existing_id = %existing_id,
                    "open slot reports an incident id that does not exist; ignoring",
                );
            }
        }
    }

    Ok(())
}

async fn handle_success(
    ctx: &PipelineContext,
    entity_qid: &str,
    report_at: DateTime<Utc>,
    scopes: &sdb::ScopeKeys,
) -> Result<(), PipelineError> {
    // Close every currently-open incident for this entity. Each open slot is
    // closed independently; NQ Closed is emitted per actually-closed
    // incident.
    let open = ctx.sdb.list_open_incidents_for_entity(entity_qid).await?;
    for (category, _id) in open {
        let outcome = ctx
            .sdb
            .close_incident(
                entity_qid,
                category,
                report_at,
                report_at,
                // close_incident reads the current report_count and stamps
                // it; passing the existing count keeps it monotonic. We
                // re-read to obtain the running total.
                running_count_for(ctx, entity_qid, category).await,
                &scopes.org_scope,
                &scopes.repo_scope,
                &scopes.env_scope,
            )
            .await?;

        if let CloseIncidentOutcome::Closed(incident) = outcome {
            info!(
                entity_qid = %entity_qid,
                category = %category,
                incident_id = %incident.id,
                "closed incident on success heartbeat"
            );
            let summary = if incident.summary.is_empty() {
                None
            } else {
                Some(incident.summary.clone())
            };
            let request = nq::NotificationRequest {
                incident_id: incident.id.to_string(),
                event_type: nq::NotificationEventType::Closed,
                entity_qid: entity_qid.to_string(),
                category: nq_category(category),
                opened_at: incident.opened_at,
                closed_at: incident.closed_at,
                summary,
            };
            ctx.nq.enqueue(&request).await?;
        }
    }
    Ok(())
}

async fn running_count_for(ctx: &PipelineContext, entity_qid: &str, category: Category) -> u64 {
    // Best-effort lookup. If the read fails, fall back to 1 — the close path
    // is otherwise correct and a slight count error is preferable to failing
    // the close.
    let Ok(Some((id, _))) = ctx.sdb.find_open_incident_id(entity_qid, category).await else {
        return 1;
    };
    match ctx.sdb.get_incident(id).await {
        Ok(Some(inc)) => inc.report_count,
        _ => 1,
    }
}

/// Convert from the `rq` re-exported category (also surfaced via `sdb`) into
/// the `nq` category. They are the same underlying type, but different
/// crates re-export under different names, so we route through the wire form
/// to keep call sites symmetric.
fn nq_category(category: IncidentCategory) -> nq::SeverityCategory {
    // [`nq::SeverityCategory`] is `pub use rq::IncidentCategory`, so this is
    // the identity mapping. Wrapping it in a function keeps the abstraction
    // explicit.
    category
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nq_category_round_trips() {
        for c in IncidentCategory::ALL {
            assert_eq!(c, nq_category(c));
        }
    }

    #[test]
    fn heartbeat_cache_round_trip() {
        let mut cache = HeartbeatCache::new();
        let now = Utc::now();
        cache.upsert(
            "Org/Repo::env@dep.0000000000000001",
            HeartbeatEntry {
                last_report_at: now,
                operational_state: Some("DESIRED".to_string()),
                volatile: false,
                watchdog_open: false,
            },
        );
        let snap = cache.snapshot();
        assert_eq!(snap.len(), 1);
        cache.remove("Org/Repo::env@dep.0000000000000001");
        assert!(cache.snapshot().is_empty());
    }

    #[test]
    fn heartbeat_cache_marks_watchdog_open() {
        let mut cache = HeartbeatCache::new();
        let now = Utc::now();
        cache.upsert(
            "Org/Repo::env@dep.0000000000000001",
            HeartbeatEntry {
                last_report_at: now,
                operational_state: Some("DESIRED".to_string()),
                volatile: false,
                watchdog_open: false,
            },
        );
        cache.mark_watchdog_open("Org/Repo::env@dep.0000000000000001");
        let entry = &cache.snapshot()[0].1;
        assert!(entry.watchdog_open);
    }
}

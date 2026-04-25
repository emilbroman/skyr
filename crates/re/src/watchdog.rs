//! Per-worker watchdog component.
//!
//! Walks the worker-local heartbeat cache periodically and opens a synthetic
//! `SystemError`-class incident for any entity whose last report is older
//! than the cadence configured for its operational state. Watchdog-opened
//! incidents are otherwise indistinguishable from producer-driven ones and
//! follow the standard close-on-success path: when a real heartbeat finally
//! arrives, the success-closes-open-incidents rule in [`crate::pipeline`]
//! takes over.
//!
//! Limitations of the in-memory cache:
//!
//! - The cache is populated by reports the worker has actually processed
//!   since startup. A fresh worker has an empty cache and only watches
//!   entities it has heard about. After a restart there is therefore a brief
//!   warm-up period during which silent failures are not detected.
//! - The cache is **not** shared between workers. Each worker watches only
//!   the entities in its shard range, which matches RQ's sharding so this
//!   property aligns with the per-entity ownership model.
//!
//! Both limitations are acceptable v1 behavior. A more durable cadence
//! tracker would require a dedicated SDB scan API or a shared cache; both
//! are out of scope.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use ids::{DeploymentQid, ResourceQid};
use rq::IncidentCategory;
use sdb::{EntityRef, OpenIncidentOutcome};
use tokio::time::{MissedTickBehavior, interval};
use tracing::{debug, info, warn};

use crate::config::WorkerConfig;
use crate::pipeline::PipelineContext;

/// Static error message used in synthetic SystemError incidents created by
/// the watchdog. Visible to operators via the SDB row's
/// `last_error_message` field and in the NE-rendered email body.
const WATCHDOG_ERROR_MESSAGE: &str =
    "watchdog: no report received within the expected cadence for this entity";

/// Spawns the watchdog as a background task. Returns the [`tokio::task::JoinHandle`]
/// so the caller can `await` shutdown on graceful exit.
pub fn spawn(ctx: PipelineContext, cfg: Arc<WorkerConfig>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = interval(cfg.watchdog_interval);
        // Avoid a flood of catch-up sweeps if the worker stalls; just resume
        // at the next regular tick.
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        // Burn the immediate tick that `interval` fires at t=0.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if let Err(error) = sweep(&ctx, &cfg).await {
                warn!(error = %error, "watchdog sweep failed");
            }
        }
    })
}

async fn sweep(
    ctx: &PipelineContext,
    cfg: &WorkerConfig,
) -> Result<(), crate::pipeline::PipelineError> {
    let snapshot = ctx.heartbeats.lock().await.snapshot();
    let now = Utc::now();
    debug!(entries = snapshot.len(), "watchdog sweep starting");
    let mut fired = 0usize;
    for (entity_qid, entry) in snapshot {
        if entry.watchdog_open {
            continue;
        }
        let Some(state) = entry.operational_state.as_deref() else {
            continue;
        };
        let Some(cadence) = cfg.cadence.for_state_str(state) else {
            continue;
        };
        let elapsed = (now - entry.last_report_at)
            .to_std()
            .unwrap_or(Duration::ZERO);
        if elapsed <= cadence {
            continue;
        }

        // Threshold tripped: try to open a synthetic SystemError incident.
        // We deliberately open at SystemError severity per the architectural
        // note: silent failure is a property of Skyr's own infrastructure.
        match try_open_watchdog_incident(ctx, &entity_qid, now).await? {
            WatchdogOutcome::Opened => {
                fired += 1;
                ctx.heartbeats.lock().await.mark_watchdog_open(&entity_qid);
            }
            WatchdogOutcome::AlreadyOpen => {
                // Another path opened a SystemError incident first; mark so
                // we stop firing until a real report clears the entry.
                ctx.heartbeats.lock().await.mark_watchdog_open(&entity_qid);
            }
            WatchdogOutcome::Skipped => {}
        }
    }
    if fired > 0 {
        info!(fired, "watchdog opened synthetic SystemError incidents");
    }
    Ok(())
}

enum WatchdogOutcome {
    Opened,
    AlreadyOpen,
    Skipped,
}

async fn try_open_watchdog_incident(
    ctx: &PipelineContext,
    entity_qid: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<WatchdogOutcome, crate::pipeline::PipelineError> {
    // We need scope keys, but the watchdog only carries the QID string.
    // Parse it back into a typed QID to derive the scopes; if parsing fails
    // (corrupt cache entry) we skip rather than crash the loop.
    let Some(scopes) = scope_keys_from_string(entity_qid) else {
        warn!(
            entity_qid,
            "watchdog skipping entry with unparseable entity qid",
        );
        return Ok(WatchdogOutcome::Skipped);
    };

    let outcome = ctx
        .sdb
        .open_incident(
            entity_qid,
            IncidentCategory::SystemError,
            now,
            WATCHDOG_ERROR_MESSAGE,
            Some(WATCHDOG_ERROR_MESSAGE.to_string()),
            &scopes.org_scope,
            &scopes.repo_scope,
            &scopes.env_scope,
        )
        .await?;
    match outcome {
        OpenIncidentOutcome::Opened(incident) => {
            info!(
                entity_qid = %entity_qid,
                incident_id = %incident.id,
                "watchdog opened synthetic SystemError incident",
            );
            let request = nq::NotificationRequest {
                incident_id: incident.id.to_string(),
                event_type: nq::NotificationEventType::Opened,
                entity_qid: entity_qid.to_string(),
                category: IncidentCategory::SystemError,
                opened_at: incident.opened_at,
                closed_at: None,
                last_error_message: Some(incident.last_error_message.clone()),
            };
            ctx.nq.enqueue(&request).await?;
            Ok(WatchdogOutcome::Opened)
        }
        OpenIncidentOutcome::AlreadyOpen { .. } => Ok(WatchdogOutcome::AlreadyOpen),
    }
}

/// Reverses [`crate::entity::scope_keys`] given only the canonical string
/// form of an entity QID. Returns `None` for strings that are neither a valid
/// deployment QID nor a valid resource QID.
fn scope_keys_from_string(entity_qid: &str) -> Option<sdb::ScopeKeys> {
    if let Ok(qid) = entity_qid.parse::<DeploymentQid>() {
        return Some(sdb::Client::scope_keys_for(EntityRef::Deployment(&qid)));
    }
    if let Ok(qid) = entity_qid.parse::<ResourceQid>() {
        return Some(sdb::Client::scope_keys_for(EntityRef::Resource(&qid)));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_scope_keys_from_deployment_string() {
        let qid = "MyOrg/MyRepo::main@2cbecbed4bfa1599ef4ce0dfc542c97a82d79268.a1b2c3d4e5f60718";
        let keys = scope_keys_from_string(qid).expect("deployment QID parses");
        assert_eq!(keys.org_scope, "MyOrg");
        assert_eq!(keys.repo_scope, "MyOrg/MyRepo");
        assert_eq!(keys.env_scope, "MyOrg/MyRepo::main");
    }

    #[test]
    fn parse_scope_keys_from_resource_string() {
        let qid = "MyOrg/MyRepo::main::Std/Random.Int:seed";
        let keys = scope_keys_from_string(qid).expect("resource QID parses");
        assert_eq!(keys.org_scope, "MyOrg");
        assert_eq!(keys.repo_scope, "MyOrg/MyRepo");
        assert_eq!(keys.env_scope, "MyOrg/MyRepo::main");
    }

    #[test]
    fn parse_scope_keys_rejects_garbage() {
        assert!(scope_keys_from_string("not a qid").is_none());
    }
}

//! Status-reporting glue for the Deployment Engine.
//!
//! This module wires the DE to the Reporting Queue (`rq`) and the Status
//! Database (`sdb`). For every iteration of the worker loop the DE constructs
//! a [`rq::Report`] describing the outcome and publishes it to the RQ. It also
//! reads two SDB signals at the top of each iteration to drive its own
//! backpressure:
//!
//! 1. [`StatusSummary::consecutive_failure_count`](sdb::StatusSummary) for the
//!    deployment, fed into the exponential-backoff formula.
//! 2. The presence of an open `Crash` incident on the deployment or any of its
//!    resources, gating whether the deployment should be attempted.
//!
//! ## Heartbeat semantics
//!
//! The DE emits a report on **every** iteration, regardless of outcome —
//! including idle "check" iterations for non-volatile bootstrapped deployments
//! and the final DOWN-state report. Reporting must not change based on the
//! current SDB state; this is a one-way producer with no feedback loop.
//!
//! ## Terminal flag
//!
//! The deployment-scoped extension carries a terminal flag that is set on the
//! last report a worker invocation will emit. That happens when the deployment
//! reaches the DOWN terminal state, and also when the worker stops its loop
//! while the deployment is still in another state but has nothing left to do
//! (e.g. a bootstrapped no-`Main.scl` deployment idling until an external
//! trigger such as a supersession respawns the worker). The RE uses this flag
//! to drop the entity from heartbeat tracking so the watchdog does not fire
//! while no reports are expected.

use std::time::Duration;

use cdb::DeploymentState;
use chrono::Utc;
use rq::{
    DeploymentExtension, DeploymentOperationalState, EntityExtension, EntityQid, IncidentCategory,
    Metrics, Outcome, Report,
};

/// What kind of failure a producer is reporting. Maps onto an
/// [`IncidentCategory`] via [`Self::category`].
///
/// The DE only emits the producer-classifiable subset of categories. `Crash`
/// is reserved for entity-aware producers (the RTE, via plugin reports) and
/// is never raised by the DE itself.
#[derive(Debug, Clone, Copy)]
pub(crate) enum FailureKind {
    /// User-supplied SCL configuration is invalid (compile errors).
    BadConfiguration,
    /// The deployment itself is stable, but reconciliation could not progress
    /// because a derived/dependent input could not be evaluated (cross-repo
    /// dep unresolved, RTP plugin error during eval, etc.).
    CannotProgress,
    /// A failure in Skyr's own infrastructure (CDB / RDB / RTQ / LDB / SDB
    /// unavailable, broker outage, etc.). Used as the safe default for
    /// otherwise-unclassified errors so threshold tuning in the RE can
    /// compensate for any miscalibration.
    SystemError,
}

impl FailureKind {
    pub(crate) fn category(self) -> IncidentCategory {
        match self {
            FailureKind::BadConfiguration => IncidentCategory::BadConfiguration,
            FailureKind::CannotProgress => IncidentCategory::CannotProgress,
            FailureKind::SystemError => IncidentCategory::SystemError,
        }
    }
}

/// Maps the lifecycle [`DeploymentState`] onto the wire-level
/// [`DeploymentOperationalState`] used in reports.
pub(crate) fn operational_state_for(state: DeploymentState) -> DeploymentOperationalState {
    match state {
        DeploymentState::Desired => DeploymentOperationalState::Desired,
        DeploymentState::Lingering => DeploymentOperationalState::Lingering,
        DeploymentState::Undesired => DeploymentOperationalState::Undesired,
        DeploymentState::Down => DeploymentOperationalState::Down,
    }
}

/// Outcome of a single worker iteration, in a form the reporter can consume
/// without having to know the producer's internal types.
#[derive(Debug, Clone)]
pub(crate) enum IterationOutcome {
    Success,
    Failure {
        kind: FailureKind,
        error_message: String,
    },
}

impl IterationOutcome {
    fn into_outcome(self) -> Outcome {
        match self {
            IterationOutcome::Success => Outcome::Success,
            IterationOutcome::Failure {
                kind,
                error_message,
            } => Outcome::Failure {
                category: kind.category(),
                error_message,
            },
        }
    }
}

/// Build a deployment-scoped [`Report`] for a single iteration.
pub(crate) fn build_deployment_report(
    deployment_qid: ids::DeploymentQid,
    state: DeploymentState,
    terminal: bool,
    elapsed: Duration,
    outcome: IterationOutcome,
) -> Report {
    let wall_time_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);

    Report {
        entity_qid: EntityQid::Deployment(deployment_qid),
        timestamp: Utc::now(),
        outcome: outcome.into_outcome(),
        metrics: Metrics::wall_time(wall_time_ms),
        extension: EntityExtension::Deployment(DeploymentExtension {
            operational_state: operational_state_for(state),
            terminal,
        }),
    }
}

/// Publish a [`Report`] to the RQ, logging publish errors at warn level.
///
/// Reporting failures must not propagate into the worker loop — the DE keeps
/// reconciling regardless of broker availability. Per the design doc, "the
/// only feedback loop from the SDB back into a producer is the DE's
/// exponential-backoff calculation; everything else stays one-way."
pub(crate) async fn publish_report(publisher: &rq::Publisher, report: &Report) {
    if let Err(error) = publisher.enqueue(report).await {
        tracing::warn!(error = %error, "failed to publish status report");
    }
}

/// Publish a terminal [`Report`] with bounded exponential-backoff retries.
///
/// The terminal flag is the only signal that lets the RE drop an entity from
/// heartbeat tracking. A lost terminal report leaves a stale cache entry that
/// the watchdog will eventually misfire on — opening a synthetic SystemError
/// incident on a deployment that is actually DOWN. Unlike the per-iteration
/// heartbeat (which gets another shot in 5s), this is the *last* report the
/// worker will emit, so we retry past a transient broker outage instead of
/// fire-and-forget.
///
/// Retries are bounded so a sustained outage cannot block worker exit
/// indefinitely; if the budget is exhausted, an error is logged and the worker
/// proceeds to shut down anyway.
pub(crate) async fn publish_terminal_report(publisher: &rq::Publisher, report: &Report) {
    const INITIAL_DELAY: Duration = Duration::from_millis(250);
    const MAX_DELAY: Duration = Duration::from_secs(30);
    const MAX_ATTEMPTS: u32 = 12;

    let mut delay = INITIAL_DELAY;
    for attempt in 1..=MAX_ATTEMPTS {
        match publisher.enqueue(report).await {
            Ok(()) => {
                if attempt > 1 {
                    tracing::info!(
                        attempts = attempt,
                        "terminal status report published after retries",
                    );
                }
                return;
            }
            Err(error) if attempt == MAX_ATTEMPTS => {
                tracing::error!(
                    error = %error,
                    attempts = attempt,
                    "giving up on terminal status report; RE watchdog may misfire on this entity",
                );
                return;
            }
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    attempt,
                    next_retry_in_ms = delay.as_millis() as u64,
                    "failed to publish terminal status report; retrying",
                );
                tokio::time::sleep(delay).await;
                delay = std::cmp::min(delay * 2, MAX_DELAY);
            }
        }
    }
}

/// SDB-derived signals read at the top of every worker iteration to drive
/// backoff and eligibility decisions.
#[derive(Debug, Clone, Default)]
pub(crate) struct SdbPreview {
    /// Latest `consecutive_failure_count` for the deployment, or `None` when
    /// the SDB has no summary row for it (entity never reported, or terminated).
    pub(crate) consecutive_failure_count: Option<u32>,
    /// `true` when the deployment itself has an open `Crash` incident.
    pub(crate) deployment_has_open_crash: bool,
    /// `true` when *any* resource owned by the deployment has an open `Crash`
    /// incident.
    pub(crate) any_resource_has_open_crash: bool,
}

impl SdbPreview {
    /// `true` when an open `Crash` incident exists for the deployment itself
    /// or any of its resources — used to gate whether the deployment should
    /// be attempted this iteration.
    pub(crate) fn any_open_crash(&self) -> bool {
        self.deployment_has_open_crash || self.any_resource_has_open_crash
    }

    /// `consecutive_failure_count` clamped to a `u32`, defaulting to 0 when
    /// SDB has no row for the deployment yet.
    pub(crate) fn failure_count(&self) -> u32 {
        self.consecutive_failure_count.unwrap_or(0)
    }
}

/// Read the deployment's status summary and the deployment-level open-`Crash`
/// flag from SDB. Errors are logged and treated as "no signal" — the DE must
/// not stall when the SDB is unreachable.
///
/// Resource-level `Crash` checks are folded in by the caller via
/// [`probe_resource_open_crash`] because the DE worker already enumerates
/// resources for other reasons.
pub(crate) async fn probe_deployment(
    sdb_client: &sdb::Client,
    deployment_qid: &ids::DeploymentQid,
) -> SdbPreview {
    let qid_str = deployment_qid.to_string();

    let consecutive_failure_count = match sdb_client.status_summary(&qid_str).await {
        Ok(Some(summary)) => Some(summary.consecutive_failure_count),
        Ok(None) => None,
        Err(error) => {
            tracing::debug!(
                deployment = %qid_str,
                error = %error,
                "failed to read SDB status summary; ignoring",
            );
            None
        }
    };

    let deployment_has_open_crash = match sdb_client
        .find_open_incident_id(&qid_str, sdb::Category::Crash)
        .await
    {
        Ok(found) => found.is_some(),
        Err(error) => {
            tracing::debug!(
                deployment = %qid_str,
                error = %error,
                "failed to query open Crash incident; ignoring",
            );
            false
        }
    };

    SdbPreview {
        consecutive_failure_count,
        deployment_has_open_crash,
        any_resource_has_open_crash: false,
    }
}

/// Returns `true` if the resource with `qid_str` has an open `Crash`
/// incident in SDB. Errors degrade to `false` so the DE never stalls.
pub(crate) async fn probe_resource_open_crash(
    sdb_client: &sdb::Client,
    resource_qid_str: &str,
) -> bool {
    match sdb_client
        .find_open_incident_id(resource_qid_str, sdb::Category::Crash)
        .await
    {
        Ok(found) => found.is_some(),
        Err(error) => {
            tracing::debug!(
                resource = %resource_qid_str,
                error = %error,
                "failed to query open Crash incident for resource; ignoring",
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operational_state_mapping_is_total() {
        assert!(matches!(
            operational_state_for(DeploymentState::Desired),
            DeploymentOperationalState::Desired,
        ));
        assert!(matches!(
            operational_state_for(DeploymentState::Lingering),
            DeploymentOperationalState::Lingering,
        ));
        assert!(matches!(
            operational_state_for(DeploymentState::Undesired),
            DeploymentOperationalState::Undesired,
        ));
        assert!(matches!(
            operational_state_for(DeploymentState::Down),
            DeploymentOperationalState::Down,
        ));
    }

    #[test]
    fn failure_kind_maps_to_category() {
        assert_eq!(
            FailureKind::BadConfiguration.category(),
            IncidentCategory::BadConfiguration,
        );
        assert_eq!(
            FailureKind::CannotProgress.category(),
            IncidentCategory::CannotProgress,
        );
        assert_eq!(
            FailureKind::SystemError.category(),
            IncidentCategory::SystemError,
        );
    }

    fn sample_qid() -> ids::DeploymentQid {
        "MyOrg/MyRepo::main@2cbecbed4bfa1599ef4ce0dfc542c97a82d79268.a1b2c3d4e5f60718"
            .parse()
            .unwrap()
    }

    #[test]
    fn build_report_carries_terminal_flag_and_state() {
        let report = build_deployment_report(
            sample_qid(),
            DeploymentState::Down,
            true,
            Duration::from_millis(123),
            IterationOutcome::Success,
        );
        match &report.extension {
            EntityExtension::Deployment(ext) => {
                assert_eq!(ext.operational_state, DeploymentOperationalState::Down);
                assert!(ext.terminal);
            }
            _ => panic!("expected deployment extension"),
        }
        assert_eq!(report.metrics.wall_time_ms, 123);
        assert!(report.outcome.is_success());
    }

    #[test]
    fn build_report_failure_carries_category() {
        let report = build_deployment_report(
            sample_qid(),
            DeploymentState::Desired,
            false,
            Duration::from_millis(10),
            IterationOutcome::Failure {
                kind: FailureKind::BadConfiguration,
                error_message: "compile error".to_string(),
            },
        );
        assert!(!report.outcome.is_success());
        assert_eq!(
            report.outcome.category(),
            Some(IncidentCategory::BadConfiguration),
        );
    }
}

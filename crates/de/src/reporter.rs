//! Status-reporting glue for the Deployment Engine.
//!
//! This module wires the DE to the Reporting Queue (`rq`) and the Status
//! Database (`sdb`). For every iteration of the worker loop the DE constructs
//! a [`rq::Report`] describing the outcome and publishes it to the RQ. It also
//! reads — but, in this iteration, does not yet *act on* — two SDB signals:
//!
//! 1. [`StatusSummary::consecutive_failure_count`](sdb::StatusSummary) for the
//!    deployment, intended to replace the legacy `cdb.failures` counter that
//!    feeds the exponential-backoff formula.
//! 2. The presence of an open `Crash` incident on the deployment or any of its
//!    resources, intended to gate whether the deployment should be attempted.
//!
//! The legacy `cdb.failures`-driven backoff path remains authoritative: this
//! module logs the SDB-derived values for observability but does not influence
//! the DE's behavior. The hard cutover that swaps the source of truth is the
//! `backoff_migration` task, which lands in lockstep with API and web changes.
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
//! single last report emitted for a deployment — when the deployment reaches
//! the DOWN terminal state. After that, no further reports are emitted.

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

/// SDB-derived signals previewed for backoff/eligibility. Read on every
/// iteration but currently used only for observability — the legacy
/// `cdb.failures` counter remains authoritative until the `backoff_migration`
/// task lands.
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
    /// Convenience accessor mirroring how the `backoff_migration` task will
    /// gate eligibility — currently observational only.
    pub(crate) fn any_open_crash(&self) -> bool {
        self.deployment_has_open_crash || self.any_resource_has_open_crash
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

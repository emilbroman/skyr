//! Entity-aware glue.
//!
//! The bulk of the RE pipeline operates on the report's common base — entity
//! QID, outcome, timestamp, metrics. Two pieces of logic must, however, look
//! at the entity-scoped extension: deciding what reporting cadence the
//! watchdog should expect, and computing the denormalized scope keys SDB needs
//! when writing an incident.
//!
//! All such logic lives here, deliberately quarantined from the transport and
//! pipeline layers so the abstraction does not leak.

use std::time::Duration;

use rq::{
    DeploymentExtension, DeploymentOperationalState, EntityExtension, EntityQid, ResourceExtension,
    ResourceOperationalState,
};
use sdb::{ScopeKeys, scope_keys_for_deployment, scope_keys_for_resource};

/// Per-state cadence configuration. The watchdog uses these durations as the
/// "no report received in over X" trigger threshold.
#[derive(Clone, Debug)]
pub struct CadenceConfig {
    pub deployment_desired: Duration,
    pub deployment_undesired: Duration,
    pub deployment_lingering: Duration,
    pub deployment_down: Option<Duration>,
    pub resource_pending: Duration,
    pub resource_live: Duration,
    pub resource_destroyed: Option<Duration>,
}

impl Default for CadenceConfig {
    /// Conservative defaults. Calibration is expected to evolve.
    fn default() -> Self {
        Self {
            // DESIRED deployments are actively reconciled by the DE; we expect
            // a report on every loop iteration.
            deployment_desired: Duration::from_secs(60),
            // UNDESIRED deployments are still being torn down but at lower
            // cadence.
            deployment_undesired: Duration::from_secs(5 * 60),
            // LINGERING deployments are essentially dormant.
            deployment_lingering: Duration::from_secs(30 * 60),
            // DOWN should be a terminal state — the entity should have been
            // deleted already. `None` means the watchdog won't fire on it.
            deployment_down: None,
            // PENDING resources are mid-creation.
            resource_pending: Duration::from_secs(60),
            // LIVE resources are checked periodically by the RTE.
            resource_live: Duration::from_secs(10 * 60),
            // DESTROYED is terminal; should already have been deleted.
            resource_destroyed: None,
        }
    }
}

impl CadenceConfig {
    /// Returns the maximum allowed gap between reports for an entity in the
    /// given cached operational state. `None` means heartbeats are not
    /// expected (terminal-ish states).
    pub fn for_state_str(&self, state: &str) -> Option<Duration> {
        // Operational state strings are the SCREAMING_SNAKE_CASE variants of
        // [`DeploymentOperationalState`] and [`ResourceOperationalState`].
        // The two enum spaces are disjoint so a single match works.
        match state {
            "DESIRED" => Some(self.deployment_desired),
            "UNDESIRED" => Some(self.deployment_undesired),
            "LINGERING" => Some(self.deployment_lingering),
            "DOWN" => self.deployment_down,
            "PENDING" => Some(self.resource_pending),
            "LIVE" => Some(self.resource_live),
            "DESTROYED" => self.resource_destroyed,
            _ => None,
        }
    }
}

/// Returns the canonical SCREAMING_SNAKE_CASE name of the operational state
/// carried in the entity-scoped extension. Cached in the SDB summary so the
/// watchdog can look up the expected cadence later.
pub fn operational_state_str(extension: &EntityExtension) -> &'static str {
    match extension {
        EntityExtension::Deployment(DeploymentExtension {
            operational_state, ..
        }) => deployment_state_str(*operational_state),
        EntityExtension::Resource(ResourceExtension {
            operational_state, ..
        }) => resource_state_str(*operational_state),
    }
}

fn deployment_state_str(state: DeploymentOperationalState) -> &'static str {
    match state {
        DeploymentOperationalState::Desired => "DESIRED",
        DeploymentOperationalState::Undesired => "UNDESIRED",
        DeploymentOperationalState::Lingering => "LINGERING",
        DeploymentOperationalState::Down => "DOWN",
    }
}

fn resource_state_str(state: ResourceOperationalState) -> &'static str {
    match state {
        ResourceOperationalState::Pending => "PENDING",
        ResourceOperationalState::Live => "LIVE",
        ResourceOperationalState::Destroyed => "DESTROYED",
    }
}

/// Computes the denormalized SDB scope keys for an entity QID.
pub fn scope_keys(qid: &EntityQid) -> ScopeKeys {
    match qid {
        EntityQid::Deployment(d) => scope_keys_for_deployment(d),
        EntityQid::Resource(r) => scope_keys_for_resource(r),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cadence_lookups_for_known_states() {
        let cfg = CadenceConfig::default();
        assert!(cfg.for_state_str("DESIRED").is_some());
        assert!(cfg.for_state_str("UNDESIRED").is_some());
        assert!(cfg.for_state_str("LINGERING").is_some());
        assert!(cfg.for_state_str("DOWN").is_none());
        assert!(cfg.for_state_str("PENDING").is_some());
        assert!(cfg.for_state_str("LIVE").is_some());
        assert!(cfg.for_state_str("DESTROYED").is_none());
        assert!(cfg.for_state_str("BOGUS").is_none());
    }

    #[test]
    fn deployment_state_strings_match_wire_format() {
        assert_eq!(
            deployment_state_str(DeploymentOperationalState::Desired),
            "DESIRED"
        );
        assert_eq!(
            deployment_state_str(DeploymentOperationalState::Undesired),
            "UNDESIRED"
        );
        assert_eq!(
            deployment_state_str(DeploymentOperationalState::Lingering),
            "LINGERING"
        );
        assert_eq!(
            deployment_state_str(DeploymentOperationalState::Down),
            "DOWN"
        );
    }

    #[test]
    fn resource_state_strings_match_wire_format() {
        assert_eq!(
            resource_state_str(ResourceOperationalState::Pending),
            "PENDING"
        );
        assert_eq!(resource_state_str(ResourceOperationalState::Live), "LIVE");
        assert_eq!(
            resource_state_str(ResourceOperationalState::Destroyed),
            "DESTROYED"
        );
    }

    #[test]
    fn operational_state_str_round_trips_for_deployment() {
        let ext = EntityExtension::Deployment(DeploymentExtension {
            operational_state: DeploymentOperationalState::Desired,
            terminal: false,
        });
        assert_eq!(operational_state_str(&ext), "DESIRED");
    }

    #[test]
    fn operational_state_str_round_trips_for_resource() {
        let ext = EntityExtension::Resource(ResourceExtension {
            operational_state: ResourceOperationalState::Live,
            terminal: false,
        });
        assert_eq!(operational_state_str(&ext), "LIVE");
    }

    #[test]
    fn scope_keys_for_deployment_qid() {
        let qid: ids::DeploymentQid =
            "MyOrg/MyRepo::main@2cbecbed4bfa1599ef4ce0dfc542c97a82d79268.a1b2c3d4e5f60718"
                .parse()
                .unwrap();
        let entity = EntityQid::Deployment(qid);
        let keys = scope_keys(&entity);
        assert_eq!(keys.org_scope, "MyOrg");
        assert_eq!(keys.repo_scope, "MyOrg/MyRepo");
        assert_eq!(keys.env_scope, "MyOrg/MyRepo::main");
    }

    #[test]
    fn scope_keys_for_resource_qid() {
        let qid: ids::ResourceQid = "MyOrg/MyRepo::main::Std/Random.Int:seed".parse().unwrap();
        let entity = EntityQid::Resource(qid);
        let keys = scope_keys(&entity);
        assert_eq!(keys.org_scope, "MyOrg");
        assert_eq!(keys.repo_scope, "MyOrg/MyRepo");
        assert_eq!(keys.env_scope, "MyOrg/MyRepo::main");
    }
}

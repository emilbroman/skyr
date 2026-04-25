//! Worker configuration. Values come from CLI flags first, with environment
//! overrides for the threshold rules and watchdog cadence so operators can
//! retune without redeploying the binary.

use std::time::Duration;

use crate::entity::CadenceConfig;
use crate::thresholds::ThresholdConfig;

/// Configuration assembled at worker startup. The threshold and cadence
/// configs are derived from environment variables; the rest comes from CLI
/// flags.
#[derive(Clone, Debug)]
pub struct WorkerConfig {
    pub thresholds: ThresholdConfig,
    pub cadence: CadenceConfig,
    pub watchdog_interval: Duration,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            thresholds: ThresholdConfig::default(),
            cadence: CadenceConfig::default(),
            watchdog_interval: Duration::from_secs(30),
        }
    }
}

impl WorkerConfig {
    /// Builds the runtime configuration, applying environment overrides where
    /// applicable.
    ///
    /// `RE_WATCHDOG_INTERVAL_SECS` overrides the watchdog sweep interval.
    pub fn from_env() -> Self {
        let interval_secs = std::env::var("RE_WATCHDOG_INTERVAL_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(30);
        Self {
            thresholds: ThresholdConfig::from_env(),
            cadence: CadenceConfig::default(),
            watchdog_interval: Duration::from_secs(interval_secs),
        }
    }
}

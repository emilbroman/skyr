use chrono::{DateTime, Utc};

use crate::category::Category;

/// Per-entity rollup row, optimized for the "list entities, show health badge"
/// API path. Lazily created on first report for an entity, and deleted on the
/// entity's terminal report (deployment reaches DOWN; resource is destroyed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusSummary {
    /// Canonical string form of the entity QID. Primary key.
    pub entity_qid: String,
    /// Timestamp of the most recent report (success or failure) seen for this
    /// entity.
    pub last_report_at: DateTime<Utc>,
    /// Whether the most recent report was a success.
    pub last_report_succeeded: bool,
    /// Number of currently-open incidents for this entity.
    pub open_incident_count: u32,
    /// Highest-severity category among open incidents; `None` when there are
    /// no open incidents.
    pub worst_open_category: Option<Category>,
    /// Consecutive failure count, fed into the DE's exponential backoff
    /// formula and into the RE's threshold rules. Resets to 0 on a successful
    /// report.
    pub consecutive_failure_count: u32,
    /// Cached operational state for the entity, encoded as an opaque string
    /// owned by the producer/RE. The SDB does not interpret this value — it
    /// only stores and returns it. Used by the RE's watchdog to know the
    /// expected reporting cadence (e.g. DESIRED vs LINGERING for deployments).
    /// `None` until the first report carries an operational state.
    pub operational_state: Option<String>,
}

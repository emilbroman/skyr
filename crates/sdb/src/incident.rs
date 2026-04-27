use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::category::Category;

/// Stable, RE-assigned identifier for an incident. Uniquely identifies an
/// incident across all entities and categories.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IncidentId(pub Uuid);

impl IncidentId {
    /// Generate a new random incident ID.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Construct from an existing [`Uuid`].
    pub const fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Get the underlying [`Uuid`].
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for IncidentId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for IncidentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

impl std::str::FromStr for IncidentId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

/// The durable, RE-owned record of a sustained failure. An incident is opened
/// when classified failure reports for an entity cross the RE's per-category
/// threshold, and closed when the threshold rules say it's over. Closure is
/// permanent — recurrence creates a brand-new incident.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Incident {
    /// RE-assigned, stable, unique ID for this incident.
    pub id: IncidentId,
    /// The entity QID this incident is about. The entity is either a deployment
    /// (a [`DeploymentQid`](ids::DeploymentQid)) or a resource
    /// (a [`ResourceQid`](ids::ResourceQid)); the SDB stores the QID as its
    /// canonical string form.
    pub entity_qid: String,
    /// The classification at *open* time. Immutable for the lifetime of the
    /// incident — incidents do not escalate or de-escalate.
    pub category: Category,
    /// Wall-clock timestamp when the threshold tripped and this incident
    /// opened.
    pub opened_at: DateTime<Utc>,
    /// Wall-clock timestamp when this incident closed; `None` while open.
    pub closed_at: Option<DateTime<Utc>>,
    /// Most recent failure report contributing to this incident. Bumped on
    /// every same-category failure observed while the incident is open.
    pub last_report_at: DateTime<Utc>,
    /// Number of failure reports observed during this incident's lifetime.
    pub report_count: u64,
    /// Cached projection of the distinct error messages observed across all
    /// reports attributed to this incident, in first-seen order, joined by
    /// `\n\n`. Each segment is truncated to [`crate::REPORT_MESSAGE_MAX_CHARS`]
    /// chars before deduping. The source of truth lives in
    /// `sdb.incident_reports`; this column is a denormalized cache so listings
    /// remain a single read.
    pub summary: String,
}

/// A single failure report attached to an incident. Stored append-only in
/// `sdb.incident_reports`, keyed on `(incident_id, report_at)` so RQ
/// redeliveries with the same wall-clock timestamp collapse idempotently.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncidentReport {
    /// Wall-clock timestamp at which the producer finished the operation.
    pub report_at: DateTime<Utc>,
    /// The error blurb the producer supplied with this report. Stored verbatim;
    /// the per-message truncation only applies when projecting into the
    /// incident's `summary`.
    pub error_message: String,
}

impl Incident {
    /// Whether this incident is currently open (no `closed_at` set).
    pub fn is_open(&self) -> bool {
        self.closed_at.is_none()
    }
}

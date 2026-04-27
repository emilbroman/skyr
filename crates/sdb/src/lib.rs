//! # Status Database (SDB) client
//!
//! SDB is the only access path to the Skyr **Status Database**: a ScyllaDB
//! keyspace that holds per-entity health summaries and incident records for
//! deployments and resources. SDB is a standalone architectural component;
//! even when it shares a physical Scylla cluster with other Skyr databases,
//! it owns its own keyspace and tables and must not reach into or be reached
//! into by other components' schemas.
//!
//! See `STATUS_REPORTING.md` for the architectural design. The contract here:
//!
//! - The Reporting Engine (`re`) is the sole writer.
//! - The API and the Deployment Engine (`de`) are readers.
//! - Entities are identified by their canonical [`ids`] QID strings; this
//!   crate does not depend on `cdb` or `rdb`.
//!
//! ## Schema overview
//!
//! - `sdb.status_summaries` — per-entity rollup, lazily created on first
//!   report and deleted on terminal report.
//! - `sdb.incidents` — authoritative incident store, partitioned by
//!   environment QID and clustered DESC by ULID-prefixed `incident_id`.
//! - `sdb.open_incidents_by_entity` — slim LWT registry that doubles as the
//!   index for `Resource.openIncidents` / `Deployment.openIncidents`. One
//!   row per `(entity_qid, category)` for as long as the incident is open.
//! - `sdb.incident_reports` — append-only per-incident report stream;
//!   source of truth from which the cached `summary` column on the
//!   `incidents` table is derived.
//!
//! ## Lifecycle invariants
//!
//! - **At most one open incident per `(entity_qid, category)` pair**, enforced
//!   via LWT (`INSERT ... IF NOT EXISTS`) on `open_incidents_by_entity`.
//! - **Closure is permanent.** Once `closed_at` is set, the incident is never
//!   re-opened. Recurrence creates a brand-new incident with a fresh id.
//! - **Status summaries are lazy** — created on first call to
//!   [`Client::upsert_status_summary`] and deleted on
//!   [`Client::delete_status_summary`].
//! - **Incident records are never deleted by this crate.** Retention/TTL is a
//!   future concern outside SDB's scope.

mod category;
mod client;
mod error;
mod incident;
mod summary;

pub use category::{Category, InvalidCategory};
pub use client::{
    Client, ClientBuilder, CloseIncidentOutcome, OpenIncidentOutcome, REPORT_MESSAGE_MAX_CHARS,
};
pub use error::{ConnectError, SdbError};
pub use incident::{Incident, IncidentId, IncidentReport};
pub use summary::StatusSummary;

#[cfg(test)]
mod tests;

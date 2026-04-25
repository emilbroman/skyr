//! Tests for the SDB crate.
//!
//! Unit tests in [`unit`] run as part of the default `cargo test`
//! invocation. They cover the value types and the client-side filtering /
//! pagination helper, neither of which require a database.
//!
//! Integration tests in [`integration`] (gated on the `scylla-tests`
//! feature) talk to a live Scylla and exercise the full read/write surface.
//! They are also marked `#[ignore]`, so they will not run on a developer
//! machine without Scylla even when the feature is enabled — invoke with:
//!
//! ```sh
//! cargo test -p sdb --features scylla-tests -- --ignored
//! ```
//!
//! By default, tests connect to `127.0.0.1:9042`. Override with the
//! `SDB_TEST_NODE` environment variable.

mod unit;

#[cfg(feature = "scylla-tests")]
mod integration;

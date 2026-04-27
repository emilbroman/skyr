# Skyr Status Database (SDB)

SDB is a library that wraps a Cassandra (ScyllaDB) client and exposes a typed API for the **Status Database** — the source of truth for per-entity health summaries and incident records produced by the [Reporting Engine](../re/) and read by the [API](../api/) and [Deployment Engine](../de/).

## Role in the Architecture

SDB is the only access path to the Status Database. Even when the underlying Scylla cluster is shared with other Skyr databases, SDB owns its own keyspace and tables and must not reach into or be reached into by other components' schemas.

```
RE → SDB ← API
     SDB ← DE
```

## Tracked Entities

For v1, SDB tracks two entity types: **deployments** and **resources**. Both are addressed exclusively by their canonical [`ids`](../ids/) QID strings (`DeploymentQid`, `ResourceQid`); SDB does not depend on `cdb` or `rdb` and does not look inside the QID — entity lifecycle is signalled to it through reports, not through cross-database reads.

## Capabilities

### Per-entity status summary

Lazy `(entity_qid)`-keyed rollup: the most recent report timestamp, whether it succeeded, the number of currently-open incidents, the highest-severity open category, the consecutive-failure count, and a cached opaque operational state used by the RE's watchdog.

- `Client::status_summary(entity_qid)` — read.
- `Client::upsert_status_summary(&summary)` — write/refresh on every report.
- `Client::delete_status_summary(entity_qid)` — clear on terminal report (deployment DOWN, resource destroyed).

### Incident records

Durable, RE-owned records of sustained failures. Each incident carries an immutable category (one of five fixed severities), `opened_at`, optional `closed_at`, a running `report_count`, and a cached `summary` projection of the failure messages observed across the incident's lifetime. The raw report stream lives in the separate `sdb.incident_reports` table; the `summary` column is a denormalized cache of `DISTINCT(error_message)`s in first-seen order, joined by `\n\n`, with each segment truncated to [`REPORT_MESSAGE_MAX_CHARS`] chars. Closure does **not** clear the summary — a closed incident retains the union of failures seen between its open and close.

The five categories ship as the [`Category`] enum and round-trip through SCREAMING_SNAKE_CASE strings in the schema:

- `BAD_CONFIGURATION`
- `CANNOT_PROGRESS`
- `INCONSISTENT_STATE`
- `SYSTEM_ERROR`
- `CRASH`

The derived `Ord` reflects severity (least → most), so `worst_open_category` is a simple `.max()` over the open set.

### Lifecycle invariants

- **At most one open incident per `(entity_qid, category)` pair.** Enforced via Scylla LWT (`INSERT ... IF NOT EXISTS`) on the `open_incidents` table. A second open of the same pair returns [`OpenIncidentOutcome::AlreadyOpen`] with the existing id; callers should append the new report to that incident instead.
- **Closure is permanent.** Once `closed_at` is set, the incident is never re-opened. Recurrence creates a brand-new incident with a fresh id.
- **Status summaries are lazy.** The row only exists between the first report and the terminal report; deletion is explicit.
- **Incident records are never deleted by this crate.** Retention/TTL is a future concern outside SDB's scope.

### Read access patterns

- `get_incident(id)` — single lookup by id; for `Organization.incident(id)` in the API.
- `incidents_by_entity(entity_qid, …)` — per-deployment / per-resource timeline.
- `incidents_by_org(org_scope, …)` / `incidents_by_repo(repo_scope, …)` / `incidents_by_env(env_scope, …)` — denormalized scope listings.

All listing methods take an [`IncidentFilter`] (`category`, `open_only`, `since`, `until`) and a [`Pagination`] (`offset`, `limit`).

The denormalized `org_scope` / `repo_scope` / `env_scope` keys are derived from the entity's QID at write time. Use the [`scope_keys_for_deployment`] / [`scope_keys_for_resource`] helpers (or `Client::scope_keys_for(EntityRef::…)`) to compute them.

## Schema

SDB manages its own keyspace (`sdb`) and tables. When changing the schema, update both the table-creation statements and the prepared statements together, and bump the schema-version comment in `client.rs`.

Tables:

- `sdb.status_summaries` — per-entity rollup row.
- `sdb.incidents_by_id` — single-incident lookup by `id` (UUID).
- `sdb.incidents_by_entity` — per-entity timeline, clustering by `(opened_at DESC, id ASC)`.
- `sdb.incidents_by_org` / `sdb.incidents_by_repo` / `sdb.incidents_by_env` — same shape as by-entity but partitioned on the denormalized scope key.
- `sdb.incident_reports` — append-only per-incident report stream, keyed `((incident_id), report_at DESC)`. Source of truth for the cached `summary` column on the incident tables.
- `sdb.open_incidents` — registry enforcing the at-most-one-open rule via LWT.

Each incident is materialized into five denormalized incident tables on open and updated coherently on append/close. Each failure report is written verbatim to `incident_reports`; on append the `summary` column is recomputed from the canonical report stream and fanned out to every denormalized row, so listings remain a single read. Listing methods combine clustering-order Scylla scans with client-side filtering for `category`, `open_only`, `since`, `until`, and `offset` / `limit`.

## Testing

Unit tests run as part of the default `cargo test` invocation and cover the value types and helpers — no Scylla required.

Integration tests are gated behind the `scylla-tests` feature **and** `#[ignore]`, so they are skipped on developer machines without a running Scylla. Run them locally with:

```sh
cargo test -p sdb --features scylla-tests -- --ignored
```

Tests share the `sdb` keyspace but isolate themselves with a per-test UUID prefix on entity QIDs and scope keys, so they can run in parallel without interfering. Override the connection with `SDB_TEST_NODE=host:port`.

## Related Crates

- [`ids`](../ids/) — QIDs that identify entities tracked by SDB.
- [`re`](../re/) — sole writer; owns threshold rules, watchdog, and notification dispatch.
- [`api`](../api/) — reader; exposes status and incidents over GraphQL.
- [`de`](../de/) — reader; uses `consecutive_failure_count` for backoff and the open-`Crash` check for eligibility.

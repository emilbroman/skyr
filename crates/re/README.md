# Skyr Reporting Engine (RE)

RE is a daemon that consumes status reports from the [RQ](../rq/), classifies
failures, maintains entity health and incident records in the [SDB](../sdb/),
and emits notification requests onto the [NQ](../nq/) when incidents open or
close.

## Role in the Architecture

RE is the entity-level health reasoner of the Skyr architecture. The
producers (DE, RTE) emit a report on every operation — success or failure —
onto the RQ. RE turns that stream into a per-entity health view by

- maintaining the SDB row that drives the API status surface,
- applying threshold rules per category to decide when a sustained failure
  becomes an incident,
- enforcing the at-most-one-open-incident-per-`(entity, category)` rule via
  Scylla LWT,
- closing open incidents when a successful heartbeat arrives, and
- emitting `NotificationRequest`s to the NQ on every incident open and close.

```
RQ → RE → SDB (health summaries + incident records)
        → NQ (notifications: open/close events)
```

RE never reads the CDB or RDB. Operational state and entity termination are
observed exclusively through report extensions.

## Topology

RE mirrors the RTQ/RTE sharding pattern: each worker owns a contiguous range
of RQ shards and is solely responsible — for the entities in its range — for
status-summary maintenance, incident-record writes, watchdog sweeps for
missing heartbeats, and notification dispatch.

Per-worker queue name follows the RQ convention:
`rq.v1.worker.{i}.of.{count}`.

## How It Works

### Per-report pipeline

1. Read the report from RQ.
2. Compute the new per-entity status summary fields and persist them to SDB.
3. **On failure:** if an incident is already open for `(entity, category)`,
   bump its counters; otherwise consult the threshold tracker — when enough
   same-category reports have arrived inside the configured window, attempt
   an LWT open in `sdb.open_incidents`. The first writer wins; the loser
   bumps the counters of the winning incident.
4. **On success:** close every open incident on the entity. Each actually-
   closed incident produces a `NotificationRequest` on NQ.
5. If the report's entity-scoped extension carries `terminal: true`, delete
   the per-entity status summary row (incident records remain).
6. Ack the RQ delivery.

### Threshold rules per category

| Category | Default threshold | Purpose |
|---|---|---|
| `Crash` | 1 report | Hair-trigger; user-visible downtime warrants immediate paging |
| `SystemError` | 3 reports / 60s | Skyr-internal failure |
| `BadConfiguration` | 5 reports / 5min | Producer-classified config rejection |
| `CannotProgress` | 5 reports / 5min | Derived/dependent config could not apply |
| `InconsistentState` | 3 reports / 2min | Reconciliation hit an irreconcilable inconsistency |

Each threshold can be overridden at startup via environment variables:

- `RE_THRESHOLD_<CATEGORY>_MIN` — minimum reports to trip
- `RE_THRESHOLD_<CATEGORY>_WINDOW_SECS` — sliding window length

…where `<CATEGORY>` is one of `BAD_CONFIGURATION`, `CANNOT_PROGRESS`,
`INCONSISTENT_STATE`, `SYSTEM_ERROR`, `CRASH`.

### Watchdog component

A worker-local task scans an in-memory cache of `(entity_qid, last_report_at,
operational_state)` every `RE_WATCHDOG_INTERVAL_SECS` seconds (default 30).
For any entry whose elapsed time exceeds the cadence configured for the
entity's operational state, it opens a synthetic `SystemError`-class
incident. The cadence defaults are:

| State (deployment) | Default grace |
|---|---|
| `DESIRED` | 60s |
| `UNDESIRED` | 5min |
| `LINGERING` | 30min |
| `DOWN` | (never — terminal) |

| State (resource) | Default grace |
|---|---|
| `PENDING` | 60s |
| `LIVE` (volatile) | 10min |
| `LIVE` (non-volatile) | (never) |
| `DESTROYED` | (never — terminal) |

Non-volatile resources (e.g. `Std/Random`, crypto keys, build artifacts) are
not re-checked by the DE after creation, so they never produce ongoing
heartbeats. The watchdog therefore treats non-volatile `LIVE` as a terminal
state and never fires on it. Volatility is carried per-report on the RQ
extension and cached alongside the operational state.

The cache is populated only by reports the worker has processed since
startup; a fresh worker has a brief warm-up window during which silent
failure detection is dampened. This is acceptable v1 behavior and matches
the worker's per-shard ownership model.

### Idempotency

The pipeline is safe under at-least-once redelivery:

- The LWT on `sdb.open_incidents` ensures duplicate "first failure" reports
  cannot produce duplicate incidents — the loser falls back to a counter
  bump on the winning incident.
- `NotificationRequest`s carry a stable `(incident_id, event_type)`
  idempotency key that NE de-duplicates on send.
- Closing an already-closed `(entity, category)` returns `NotOpen` and emits
  no further `Closed` notification.

## Running

```sh
cargo run -p re -- daemon \
  --region stockholm \
  --domain skyr.cloud \
  --worker-index 0 \
  --worker-count 4 \
  --local-workers 1
```

Peer service addresses (`rq`, `sdb`, `nq`) are resolved from `--region` and
`--domain` via the canonical `<service>.<region>.int.<domain>` template
(see `ids::service_address`).

Multiple RE workers can run in parallel with disjoint shard ranges. Use
`--worker-index` and `--local-workers` to assign one or more shard ranges to
a single process.

## Related Crates

- [RQ](../rq/) — source of report messages
- [SDB](../sdb/) — health summaries and incident records
- [NQ](../nq/) — outgoing notification requests
- [NE](../ne/) — consumer of NQ; performs SMTP delivery

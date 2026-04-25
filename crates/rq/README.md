# Skyr Reporting Queue (RQ)

RQ is a library that wraps an AMQP (RabbitMQ) client and exposes a typed API
for the reporting message queue. It carries `Report` messages from the various
Skyr engines (DE, RTE) to the Reporting Engine (RE), which uses them to
maintain per-entity status and incident records in the Status Database (SDB).

## Role in the Architecture

```
DE/RTE → RQ → RE → SDB
                 → NQ → NE → SMTP
```

Producers (DE, RTE) emit a `Report` for **every** operation — success or
failure. Successful reports are heartbeats; failure reports carry a producer-
assigned classification category that drives the RE's threshold rules.

## Report shape

Every `Report` carries:

- An entity QID — either a `DeploymentQid` or a `ResourceQid`. The entity's
  string form is the shard-routing key.
- A UTC timestamp.
- An `Outcome` — either `Success`, or `Failure { category, error_message }`.
  The five severity categories (`SystemError`, `BadConfiguration`,
  `CannotProgress`, `InconsistentState`, `Crash`) are defined here in the `rq`
  crate and re-exported by downstream consumers (notably `sdb` and `re`).
- A `Metrics` envelope: a known `wall_time_ms` field plus an open `extra` map
  for forward-compatible additions. v1 producers only fill `wall_time_ms`.
- An `EntityExtension` — a typed enum carrying entity-specific fields. v1
  variants are `Deployment` and `Resource`. The extension carries the entity's
  current operational state and a `terminal` flag that signals end of entity
  lifecycle so the RE can delete the per-entity summary row.

The transport layer is **deliberately agnostic** to the entity-scoped
extension: shard routing, publishing, and consumption never pattern-match on
the extension variant. Adding a new entity type later requires only a new
extension variant — no changes to transport code.

## Implementation

- Reports are JSON-encoded via serde.
- Producers use `ClientBuilder::build_publisher()` and call
  `Publisher::enqueue(&Report)`.
- Workers use `ClientBuilder::build_consumer(WorkerConfig)` and consume typed
  deliveries. Each delivery exposes `ack()` and `nack(requeue)` for explicit
  acknowledgement.

### Topology

The AMQP topology mirrors RTQ (the cluster is fully controlled, so it is
hard-coded):

- Direct exchange (`rq.v1`).
- 32 shards for parallelism.
- Routing key derived from a deterministic SipHash of the entity QID's string
  form, so all reports for a given entity always land on the same shard and
  therefore the same RE worker.
- Worker queue bindings derived from `WorkerConfig` shard ownership — each
  worker is assigned a contiguous subset of shards.

### Dead-letter / retry strategy

Like RTQ, RQ does not declare a dead-letter exchange in application code —
broker-side configuration is expected to attach a DLX to messages that exceed
retry limits. The RE consumer `nack`s without requeue (`requeue = false`) for
permanent failures.

## Related Crates

- [DE](../de/), [RTE](../rte/) — publish reports
- [RE](../re/) — consumes reports, maintains SDB and emits NQ messages
- [SDB](../sdb/) — re-uses `IncidentCategory` from this crate

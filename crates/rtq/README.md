# Skyr Resource Transition Queue (RTQ)

This library wraps an AMQP client and exposes a typed API for interacting with the resource transition queue.

## Current Stub Scope

- Messages are typed as the `Message` enum (`CREATE`, `RESTORE`, `ADOPT`, `DESTROY`).
- Messages are JSON-encoded via `serde`.
- AMQP details are encapsulated:
  - Producers use `ClientBuilder::build_publisher()` and call `Publisher::enqueue(&Message)`.
  - Workers use `ClientBuilder::build_consumer(WorkerConfig)` and consume typed deliveries.
- Topology is hard-coded internally (cluster is fully controlled) and shard-based:
  - Direct exchange (`rtq.v1` by default).
  - Routing key derived from hashed resource UID.
  - Worker queue bindings derived from `WorkerConfig` shard ownership.

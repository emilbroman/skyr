# Skyr Resource Transition Engine (RTE)

RTE is a daemon that consumes transition messages from the [RTQ](../rtq/) and executes resource modifications by invoking plugins via the [RTP](../rtp/) protocol.

## Role in the Architecture

RTE is the workhorse that actually creates, updates, and destroys resources. It translates transition intents from the queue into plugin calls, then persists the results in the [RDB](../rdb/).

```
RTQ → RTE → RTP (plugin calls)
             RTE → RDB (persist state)
             RTE → LDB (write logs)
             RTE → RQ  (status reports)
```

## How It Works

RTE connects to RTQ as a consumer with configurable worker shards and dials RTP plugins specified via `--plugin NAME@TARGET` CLI arguments.

### Message Processing

| Message | Behavior |
|---------|----------|
| **Create** | Calls plugin `create_resource`, persists inputs/outputs/dependencies to RDB. Drops duplicate creates for existing resources (idempotent). |
| **Destroy** | Validates the requesting deployment owns the resource, calls plugin `delete_resource`, removes from RDB. Drops deletes for missing or non-owned resources. |
| **Adopt** | Transfers ownership in RDB. If inputs differ from the existing resource, calls plugin `update_resource`. |
| **Restore** | Compares desired inputs against current state. If they differ, calls plugin `update_resource` to re-apply desired state. |

### RDB Persistence

Create messages persist state to RDB in multiple steps: inputs and dependencies first, then outputs after the plugin responds. This ensures partial state is visible during long-running plugin operations.

### Error Handling

If a plugin call or RDB operation fails, the message is nack'd (not acknowledged), allowing RabbitMQ to redeliver it. This provides at-least-once delivery semantics for all transition operations.

### Idempotency

- Duplicate Create messages for existing resources are dropped.
- Destroy messages for missing or non-owned resources are dropped.

### Logging

All transition events are logged to deployment QID-keyed topics via the [LDB](../ldb/). The deployment QID (e.g., `org/repo::env@deploy`) is used as the log namespace.

### Status reporting

After every processed resource transition (Create, Restore, Adopt, Destroy, Check), RTE publishes a single [RQ](../rq/) report carrying the resource QID, wall-clock duration, success/failure outcome (with a producer-assigned `IncidentCategory` and error message on failure), and a resource-scoped extension recording the operational state (`Pending`, `Live`, `Destroyed`) and a `terminal` flag set on a successful destroy. Reporting is unconditional for transitions that actually run; idempotent message drops (already-exists creates, missing-resource destroys/checks, owner mismatches) do not emit reports because no transition was performed.

Reports are best-effort from the producer's perspective — RQ publish failures are logged at warn-level but do not affect the transition outcome itself.

## Running

```sh
cargo run -p rte -- daemon \
  --plugin "Std/Random@localhost:50051" \
  --plugin "Std/Time@localhost:50056" \
  --plugin "Std/Artifact@localhost:50052" \
  --plugin "Std/DNS@localhost:50057" \
  --plugin "Std/HTTP@localhost:50058" \
  --plugin "Std/Container@localhost:50053"
```

Multiple RTE workers can run in parallel, each handling a subset of RTQ shards. Use `--worker-index` and `--worker-count` to configure shard ownership.

## Related Crates

- [RTQ](../rtq/) — source of transition messages
- [RTP](../rtp/) — protocol for invoking plugins
- [RDB](../rdb/) — resource state persistence
- [LDB](../ldb/) — transition event logging
- [RQ](../rq/) — status report sink consumed by the Reporting Engine

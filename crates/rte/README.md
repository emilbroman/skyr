# Skyr Resource Transition Engine (RTE)

RTE is a daemon that consumes transition messages from the [RTQ](../rtq/) and executes resource modifications by invoking plugins via the [RTP](../rtp/) protocol.

## Role in the Architecture

RTE is the workhorse that actually creates, updates, and destroys resources. It translates transition intents from the queue into plugin calls, then persists the results in the [RDB](../rdb/).

```
RTQ → RTE → RTP (plugin calls)
             RTE → RDB (persist state)
             RTE → LDB (write logs)
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

### Idempotency

- Duplicate Create messages for existing resources are dropped.
- Destroy messages for missing or non-owned resources are dropped.

### Logging

All transition events are logged to deployment log topics via the [LDB](../ldb/).

## Running

```sh
cargo run -p rte -- daemon \
  --plugin "Std/Random@localhost:50051" \
  --plugin "Std/Artifact@localhost:50052" \
  --plugin "Std/Container@localhost:50053"
```

Multiple RTE workers can run in parallel, each handling a subset of RTQ shards.

## Related Crates

- [RTQ](../rtq/) — source of transition messages
- [RTP](../rtp/) — protocol for invoking plugins
- [RDB](../rdb/) — resource state persistence
- [LDB](../ldb/) — transition event logging

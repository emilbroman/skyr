# Skyr Resource Transition Queue (RTQ)

RTQ is a library that wraps an AMQP (RabbitMQ) client and exposes a typed API for the resource transition message queue.

## Role in the Architecture

RTQ is the communication channel between the [DE](../de/) (which decides what transitions are needed) and the [RTE](../rte/) (which executes them). Messages are sharded across workers for parallel processing.

```
DE → RTQ → RTE
```

## Message Types

| Message | Description |
|---------|-------------|
| **Create** | Create a new resource |
| **Restore** | Re-apply desired state to a drifted resource |
| **Adopt** | Transfer resource ownership between deployments |
| **Destroy** | Delete a resource no longer needed |

Each message contains a `ResourceRef` with `environment_qid`, resource type, and resource name. Additional fields per message type:

- **Create**: `owner_deployment_qid`, `desired_inputs`, `dependencies`
- **Restore**: `owner_deployment_qid`, `desired_inputs`, `dependencies`
- **Adopt**: `from_owner_deployment_qid`, `to_owner_deployment_qid`, `desired_inputs`, `dependencies`
- **Destroy**: `owner_deployment_qid`

## Implementation

- Messages are typed as the `Message` enum and JSON-encoded via serde.
- Producers use `ClientBuilder::build_publisher()` and call `Publisher::enqueue(&Message)`.
- Workers use `ClientBuilder::build_consumer(WorkerConfig)` and consume typed deliveries. Each delivery exposes `ack()` and `nack(requeue)` methods for explicit acknowledgment.

### Topology

The AMQP topology is hard-coded (the cluster is fully controlled):

- Direct exchange (`rtq.v1`).
- 32 shards for parallelism.
- Routing key derived from a consistent hash of the resource UID (`environment_qid:resource_type:resource_name`).
- Worker queue bindings derived from `WorkerConfig` shard ownership — each worker is assigned a subset of shards.

## Related Crates

- [DE](../de/) — publishes transition messages
- [RTE](../rte/) — consumes and processes transition messages
- [RTP](../rtp/) — protocol used by RTE to invoke plugins

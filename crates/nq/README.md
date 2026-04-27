# Skyr Notification Queue (NQ)

NQ is a library that wraps an AMQP (RabbitMQ) client and exposes a typed API for the notification request message queue.

## Role in the Architecture

NQ is the communication channel between the [RE](../re/) (Reporting Engine) and the [NE](../ne/) (Notification Engine). The RE writes a `NotificationRequest` onto NQ when an incident opens or closes, and the NE consumes the queue and performs the SMTP delivery.

```
RE → NQ → NE
```

Decoupling notification dispatch from the RE means SMTP outages cannot back up the report-processing path on RQ, and notification throughput can scale independently.

## Message Type

A single message type, `NotificationRequest`, with the following fields:

| Field | Description |
|-------|-------------|
| `incident_id` | RE-assigned, stable, unique incident identifier |
| `event_type` | `OPENED` or `CLOSED` |
| `entity_qid` | Stringified QID of the affected entity (deployment or resource) |
| `category` | Severity category — one of `SYSTEM_ERROR`, `BAD_CONFIGURATION`, `CANNOT_PROGRESS`, `INCONSISTENT_STATE`, `CRASH` |
| `opened_at` | RFC3339 timestamp when the incident was opened |
| `closed_at` | RFC3339 timestamp when the incident closed (only on `CLOSED` events) |
| `summary` | Projected incident summary — distinct error messages joined by `\n\n` — for the email body (optional) |

The pair `(incident_id, event_type)` is the **stable idempotency key**, exposed via `NotificationRequest::idempotency_key()`. NQ also stamps that key into the AMQP `message_id` property of every published delivery.

## Implementation

- Producers use `ClientBuilder::build_publisher().await` and call `Publisher::enqueue(&NotificationRequest)`.
- Consumers use `ClientBuilder::build_consumer().await` and pull deliveries via `Consumer::next()`. Each delivery exposes `ack()` and `nack(requeue)` for explicit acknowledgment.

### Topology

The AMQP topology is hard-coded (the cluster is fully controlled):

- Direct exchange `nq.v1`, durable.
- Single durable queue `nq.v1`, bound to the exchange with the empty routing key.
- **No sharding.** Notifications are independent events; multiple NE replicas pull from the same queue under plain competing-consumer semantics.
- Optional dead-letter exchange (DLX) configurable via `ClientBuilder::dead_letter_exchange()`. The DLX itself is **not** declared by this crate — operations declares it.

### Delivery Semantics

- **At-least-once.** Messages are persistent (delivery mode 2) and publishes are awaited until the broker confirms them.
- **No per-entity ordering.** The NQ is unsharded by design; if two notifications for the same entity arrive close together, NE replicas may handle them in any order.
- **NE de-duplicates.** The receiver is responsible for honoring the idempotency key so that at-least-once redelivery does not double-send emails. NQ does not perform dedup itself.

## Related Crates

- [RE](../re/) — produces notification requests on incident open/close transitions.
- [NE](../ne/) — consumes notification requests and performs SMTP delivery.
- [RQ](../rq/) — Reporting Queue. Owns the upstream report payload and (per a Wave 1 reconciliation) the canonical `SeverityCategory` enum that NQ also re-uses on its wire payload.

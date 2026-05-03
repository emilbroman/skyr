# Skyr Notification Engine (NE)

NE is the daemon that consumes the [NQ](../nq/) (Notification Queue) and
delivers notification e-mails over SMTP. It is the **only** Skyr component that
holds SMTP credentials.

## Role in the Architecture

```
RE → NQ → NE → SMTP → operators
```

The Reporting Engine ([RE](../re/)) writes `NotificationRequest`s to NQ when an
incident opens or closes; NE pulls them off and sends e-mail. Decoupling the
two means SMTP outages cannot back up the report-processing path on RQ.

NE has **no knowledge** of the incident state machine. It reacts only to events
on the queue; it does not query the SDB and does not understand thresholds.

## Per-Message Pipeline

For each [`nq::Delivery`]:

1. **Dedup claim.** Compute the request's stable idempotency key
   (`incident_id:opened|closed`) and `SET ... NX EX` it in Redis. If the key
   was already claimed, ack the redelivery and stop — this is how at-least-once
   redeliveries avoid producing duplicate user-visible emails.
2. **Resolve recipients at consume time** by extracting the owning [`ids::OrgId`]
   from the entity QID (deployment QID or resource QID) and listing org members
   via [UDB](../udb/). NE does **not** snapshot the recipient list at RE-enqueue
   time — membership changes between RE enqueue and NE dispatch take effect
   immediately.
3. **Enqueue into the per-recipient batcher** (one push per resolved recipient,
   then ack the AMQP delivery). The batcher is described below.
4. **Flush** is performed asynchronously by a background task: render each
   queued event, concatenate the bodies into one e-mail, and send via SMTP
   (lettre, async tokio + rustls).
   - Success → drop.
   - Transient or permanent SMTP failure → log and drop (the AMQP delivery
     was already acked). The durable record remains in [SDB](../sdb/);
     operators can still inspect incidents via the API even if the
     notification e-mail was lost.

There is no dead-letter routing for flush-time SMTP failures because
the batched e-mail represents many original deliveries: re-queueing it
in any sensible form would either resurrect already-acked messages or
duplicate-deliver others. Operational visibility for sustained SMTP
failures should come from logs and SMTP-side metrics, not from the
broker DLX.

## Per-Recipient Batcher

NE coalesces notifications per recipient through an in-process batcher
to keep an upstream loop bug from becoming an e-mail flood. The batcher
is keyed on the recipient's e-mail address and applies two timers and a
size cap:

| Knob | Default | Meaning |
|---|---|---|
| `quiet` | 10 s | Sliding timer; resets on every new event for the recipient. |
| `cap` | 10 min | Hard upper bound from the first event in the batch; never extended. |
| `max_batch_size` | 5000 | Hard size cap; forces an early flush when reached. |

A batch flushes when *either* deadline passes (whichever comes first)
or when the size cap is reached. Under normal operation each batch
holds exactly one event and flushes after the `quiet` window elapses,
so the only steady-state cost is the 10-second flush latency. During a
runaway upstream the batcher caps each recipient at one e-mail per
`cap` window, with the body containing the concatenated rendering of
every queued event so operators retain full diagnostic detail.

Single-event batches are rendered identically to a non-batched email so
isolated notifications look the same as before. Multi-event batches use
a count-style subject:

```
[Skyr] 47 incident events (24 opened, 23 closed)
```

The batcher has **no knowledge** of incident state — the subject
counts are derived purely from `event_type`. NE's "no incident state
machine" invariant is preserved.

The batcher lives in-process per NE instance. Multi-instance
deployments lose perfect coalescing across instances — recipient `R`'s
notifications round-robin across consumers and each instance batches
independently — but the steady-state worst case is one e-mail per
`cap` window per NE instance, which is well under the unmitigated rate
during an upstream loop.

On graceful shutdown (SIGTERM / SIGINT), every in-flight batch is
flushed synchronously before the process exits. Crash-time loss of
in-flight batches is the cost of the ack-at-consume design and is
preferred over holding AMQP deliveries unacked for up to ten minutes.

## Configuration

All flags are accepted by `ne daemon` and a select few also via environment
variables:

| Flag | Env var | Description |
|------|---------|-------------|
| `--region` |  | Skyr region this NE serves (e.g. `stockholm`); `[a-z]+` |
| `--service-address-template` |  | Substitution template for region-scoped peer addressing (default: `{service}.{region}.int.skyr.cloud`); placeholders are `{service}` and `{region}` |
| `--nq-uri` |  | Override the full AMQP URI for NQ (otherwise resolved by substituting `service=nq` and the local region into `--service-address-template` and prefixing `amqp://` / suffixing `:5672/%2f`) |
| `--nq-dlx` |  | Optional DLX to attach to the NQ queue declaration |
| `--nq-dlx-routing-key` |  | Routing key for the DLX |
| `--prefetch` |  | AMQP basic.qos prefetch (default 4) |
| `--worker-count` |  | Concurrent worker tasks pulling from the same queue (default 1) |
| `--dedup-hostname` |  | Redis host for the dedup ledger (default `localhost`) |
| `--dedup-ttl-seconds` |  | TTL on each dedup claim (default 7 days) |
| `--smtp-host` |  | SMTP server hostname (required) |
| `--smtp-port` |  | SMTP port (default 587) |
| `--smtp-tls` |  | `starttls` (default), `tls`, or `none` |
| `--smtp-username` | `NE_SMTP_USERNAME` | SMTP AUTH username (optional) |
| `--smtp-password` | `NE_SMTP_PASSWORD` | SMTP AUTH password (env var preferred) |
| `--smtp-from` |  | Sender mailbox, e.g. `Skyr <skyr@example.com>` (required) |
| `--smtp-timeout-seconds` |  | Connection timeout (default 30) |

UDB is resolved by substituting `service=udb` and the local region into
`--service-address-template` (`ids::ServiceAddressTemplate::format`). NE is
the last non-IAS holdout still talking to UDB directly — the recipient
resolution path predates IAS and is expected to migrate to an IAS RPC
later. The `--dedup-hostname` flag remains a direct Redis hostname, since
the dedup store is a backing service rather than a Skyr peer.

## Why Redis for Dedup

The dedup ledger only needs `SET key value EX ttl NX` and a self-cleaning TTL.
Redis is already a load-bearing dependency in this codebase (`udb`, `scs`,
`scoc`, several plugins), so there is no reason to spin up a Scylla keyspace
just for one tiny ledger. The crate-level note in `tasks/status-reporting/ne_engine.md`
also calls out Redis as the preferred choice when one is already in use.

## Related Crates

- [NQ](../nq/) — message transport.
- [UDB](../udb/) — org membership and user lookups.
- [RE](../re/) — produces the notification requests NE consumes.

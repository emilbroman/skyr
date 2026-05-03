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
2. **Resolve recipients at send time** by extracting the owning [`ids::OrgId`]
   from the entity QID (deployment QID or resource QID) and listing org members
   via [UDB](../udb/). NE does **not** snapshot the recipient list at enqueue
   time — membership changes between RE enqueue and NE dispatch take effect
   immediately.
3. **Render** subject + plain-text body. Templates are deliberately small in v1;
   richer HTML templating is out of scope.
4. **Send** via SMTP (lettre, async tokio + rustls).
   - Success → ack.
   - Transient failure → release the dedup claim, `nack(requeue=true)`. A
     redelivery will retry.
   - Permanent failure → leave the dedup claim, `nack(requeue=false)` so the
     broker DLX (if configured) catches the message for postmortem.

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

# Skyr Log Database (LDB)

LDB is a library that wraps a Kafka (Redpanda) client and provides structured logging scoped to deployment namespaces.

## Role in the Architecture

LDB provides deployment-scoped logging for services that need to record and stream operational events. It is written to by the [RTE](../rte/) and other services, and read by the [API](../api/) for log access.

```
DE  → LDB (Kafka/Redpanda) ← API
RTE → LDB                  ← API
SCOC → LDB
```

## Capabilities

### Publishing

- `NamespacePublisher` with convenience methods: `info()`, `warn()`, `error()`, `log()`.
- Use the deployment ID as the namespace for deployment-scoped logs.

### Consuming

- `NamespaceConsumer` with a `tail()` streaming method.
- `TailConfig` supports follow mode and configurable start position (from end or beginning).

### Severity Levels

| Level | Description |
|-------|-------------|
| Info | Informational events |
| Warning | Non-critical issues |
| Error | Failures requiring attention |

## Internals

- Topic names follow the format `dl-{base64(namespace)}`.
- Binary payload format: 8-byte timestamp + 1-byte severity + UTF-8 message.

## Client Construction

Clients are created via a builder pattern:

- `ClientBuilder::brokers(addr)` → `Publisher` / `Consumer`
- `Publisher::namespace(qid)` → `NamespacePublisher`
- `Consumer::namespace(qid)` → `NamespaceConsumer`

## Related Crates

- [DE](../de/) — logs compile diagnostics and deployment events
- [RTE](../rte/) — logs transition events
- [SCOC](../scoc/) — streams container logs
- [API](../api/) — exposes logs to users

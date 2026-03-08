# Skyr Log Database (LDB)

LDB is a library that wraps a Kafka (Redpanda) client and provides structured logging scoped to deployment namespaces.

## Role in the Architecture

LDB provides deployment-scoped logging for services that need to record and stream operational events. It is written to by the [RTE](../rte/) and other services, and read by the [API](../api/) for log access.

```
RTE → LDB (Kafka/Redpanda) ← API
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

## Related Crates

- [RTE](../rte/) — logs transition events
- [API](../api/) — exposes logs to users

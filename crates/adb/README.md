# Skyr Artifact Database (ADB)

ADB is a library that wraps an S3 client and exposes a typed API for storing and retrieving deployment artifacts.

## Role in the Architecture

ADB provides artifact storage for plugins (e.g., the [artifact plugin](../plugin_std_artifact/)) and artifact access for the [API](../api/).

```
Plugin → ADB (S3/MinIO) ← API
```

## Capabilities

| Operation | Description |
|-----------|-------------|
| `write` | Store an artifact with namespace/name key and media type |
| `read` / `read_to_bytes` | Retrieve an artifact |
| `read_header` | Read artifact metadata without the body |
| `list` | List all artifacts in a namespace |
| `presign_read_url` | Generate a time-limited presigned URL for external access |
| `private_read_url` | Generate an internal URL for service-to-service access |

Writes support `if-none-match` for idempotent creates (conditional write).

## Configuration

Endpoint, bucket, credentials, and region are configurable via CLI arguments or environment variables.

In local development, MinIO is used as the S3-compatible backend (ports 9000/9001).

## Related Crates

- [plugin_std_artifact](../plugin_std_artifact/) — writes artifacts via ADB
- [API](../api/) — exposes artifacts to users

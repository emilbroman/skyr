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
| `read_url` | Generate the public (non-presigned) URL for an artifact |

Writes support `if-none-match` for idempotent creates (conditional write).

## Client Construction

Clients are created via `ClientBuilder` with the following configuration:

- `bucket()` — S3 bucket name
- `endpoint_url()` — internal S3 endpoint used for read/write SDK calls
- `external_url()` — public-facing base URL surfaced to users (used by both `presign_read_url` and `read_url`); defaults to `endpoint_url()` when unset
- `region()`, `access_key_id()`, `secret_access_key()` — AWS credentials
- `force_path_style()` — use path-style addressing (required for MinIO)
- `create_bucket_if_missing()` — auto-create the bucket on startup

In local development, MinIO is used as the S3-compatible backend (ports 9000/9001).

## Related Crates

- [plugin_std_artifact](../plugin_std_artifact/) — writes artifacts via ADB
- [API](../api/) — exposes artifacts to users

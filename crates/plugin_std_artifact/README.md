# Std/Artifact Plugin

An [RTP](../rtp/) plugin implementing the `Std/Artifact.File` resource type, backed by [ADB](../adb/).

## Role in the Architecture

This plugin manages artifact files in S3-compatible storage. It is invoked by the [RTE](../rte/) when deployments use `Artifact.File` resources.

## Resource: `Std/Artifact.File`

Stores a file as a named artifact.

| | Fields |
|---|--------|
| **Inputs** | `namespace`, `name`, `contents`, optional `type` (media type) |
| **Outputs** | `namespace`, `name`, `media_type`, `url` (private URL) |

Creates are idempotent — if an artifact with the same name already exists, it is treated as success. Deletes are a no-op (artifacts are retained indefinitely).

## Running

```sh
cargo run -p plugin_std_artifact -- --bind 0.0.0.0:50052
```

## Related Crates

- [RTP](../rtp/) — the plugin protocol this implements
- [RTE](../rte/) — invokes this plugin to process transitions
- [ADB](../adb/) — artifact storage backend

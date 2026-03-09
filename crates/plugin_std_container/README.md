# Std/Container Plugin

An [RTP](../rtp/) plugin implementing container resource types (`Std/Container.Image`, `Std/Container.Pod`, `Std/Container.Pod.Container`), using [SCOP](../scop/) to communicate with [SCOC](../scoc/) conduit nodes.

## Role in the Architecture

This is the most complex standard library plugin. It manages the full container lifecycle: building images, creating pod sandboxes, and running containers on cluster nodes.

```
RTE → RTP → Container Plugin → SCOP → SCOC → containerd
                              → BuildKit (image builds)
                              → OCI Registry (push/pull images)
                              → CDB (read repo contents)
                              → LDB (write logs)
                              → Node Registry (discover nodes)
```

## Resource Types

### `Std/Container.Image`

Builds a container image from a Git context using BuildKit and pushes it to the OCI registry. The resource namespace (a deployment QID like `org/repo::env@deploy`) is parsed to locate the correct repository and commit in CDB for extracting the build context.

### `Std/Container.Pod`

Creates a pod sandbox on a worker node via SCOP.

### `Std/Container.Pod.Container`

Creates and starts a container within a pod via SCOP.

## Node Discovery

The plugin looks up conduit node addresses from the node registry (Redis). [SCOC](../scoc/) instances register themselves on startup.

## Running

```sh
cargo run -p plugin_std_container -- \
  --bind 0.0.0.0:50053 \
  --rtp-bind 0.0.0.0:50053 \
  --node-registry-hostname localhost \
  --cdb-hostnames localhost \
  --buildkit-addr tcp://localhost:1234 \
  --registry-url http://localhost:5000
```

## Related Crates

- [IDs](../ids/) — parses deployment QIDs from resource namespaces
- [RTP](../rtp/) — the plugin protocol this implements
- [RTE](../rte/) — invokes this plugin to process transitions
- [SCOP](../scop/) — protocol for communicating with conduit nodes
- [SCOC](../scoc/) — conduit nodes that run containers
- [CDB](../cdb/) — reads repository contents for image builds
- [LDB](../ldb/) — writes build and deployment logs

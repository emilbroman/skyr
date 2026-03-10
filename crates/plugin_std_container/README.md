# Std/Container Plugin

An [RTP](../rtp/) plugin implementing container resource types, using [SCOP](../scop/) to communicate with [SCOC](../scoc/) conduit nodes.

## Role in the Architecture

This is the most complex standard library plugin. It manages the full container lifecycle: building images, creating pod sandboxes, running containers, and exposing ports on cluster nodes. It also serves as the SCOP Orchestrator, handling node registration and heartbeats.

```
RTE → RTP → Container Plugin → SCOP → SCOC → containerd
                              → BuildKit (image builds)
                              → OCI Registry (push/pull images)
                              → CDB (read repo contents)
                              → LDB (write logs)
       SCOC → SCOP → Container Plugin (Orchestrator: register/heartbeat)
```

## Resource Types

### `Std/Container.Image`

Builds a container image from a Git context using BuildKit and pushes it to the OCI registry. The resource namespace (a deployment QID like `org/repo::env@deploy`) is parsed to locate the correct repository and commit in CDB for extracting the build context.

| | Fields |
|---|--------|
| **Inputs** | `name`, `context` (path relative to repo root), `containerfile` (path relative to context) |
| **Outputs** | `fullname` (full image reference with digest), `digest` |

Updates trigger a full rebuild. Images are not deleted from the registry on resource deletion (garbage collected by registry policies).

### `Std/Container.Pod`

Creates a pod sandbox on a worker node via SCOP.

| | Fields |
|---|--------|
| **Inputs** | `name`, `uid`, optional `node` (auto-scheduled if omitted), optional `allow` (egress allow-list of port resources) |
| **Outputs** | `podId`, `node`, `name`, `address` (pod IP) |

Pods are immutable in CRI — changes to `name`, `uid`, or `allow` trigger pod recreation.

### `Std/Container.Pod.Container`

Creates and starts a container within a pod via SCOP.

| | Fields |
|---|--------|
| **Inputs** | `podId`, `podName`, `podUid`, `node`, `name`, `image`, optional `command`, optional `args`, optional `envs` |
| **Outputs** | `containerId`, `name`, `image` |

Containers are immutable — any input change triggers deletion and recreation. Containers are automatically started after creation.

### `Std/Container.Pod.Port`

Opens an ingress firewall port on a pod via SCOP.

| | Fields |
|---|--------|
| **Inputs** | `podId`, `podAddress`, `node`, `port`, `protocol`, optional `name` |
| **Outputs** | `address`, `port`, `protocol` |

Ports are immutable — any input change triggers recreation.

## Node Discovery

The plugin serves as the SCOP Orchestrator. [SCOC](../scoc/) conduit nodes register on startup, reporting their capacity and receiving a pod CIDR assignment. The plugin tracks registered nodes and schedules pods to nodes when no specific node is requested.

## Running

```sh
cargo run -p plugin_std_container -- \
  --bind 0.0.0.0:50053 \
  --rtp-bind tcp://0.0.0.0:50054 \
  --node-registry-hostname localhost \
  --cdb-hostnames localhost \
  --buildkit-addr tcp://localhost:1234 \
  --registry-url http://localhost:5000 \
  --ldb-hostname 127.0.0.1 \
  --cluster-cidr 10.42.0.0/16
```

## Related Crates

- [IDs](../ids/) — parses deployment QIDs from resource namespaces
- [RTP](../rtp/) — the plugin protocol this implements
- [RTE](../rte/) — invokes this plugin to process transitions
- [SCOP](../scop/) — protocol for communicating with conduit nodes and serving orchestrator
- [SCOC](../scoc/) — conduit nodes that run containers
- [CDB](../cdb/) — reads repository contents for image builds
- [LDB](../ldb/) — writes build and deployment logs

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

### `Std/Container.Pod.Port`

Opens an ingress firewall port on a pod via SCOP.

| | Fields |
|---|--------|
| **Inputs** | `podId`, `podAddress`, `node`, `port`, `protocol`, optional `name` |
| **Outputs** | `address`, `port`, `protocol` |

Ports are immutable — any input change triggers recreation.

## Node Discovery

The plugin serves as the SCOP Orchestrator. [SCOC](../scoc/) conduit nodes register on startup, reporting their capacity and receiving a pod CIDR assignment. The plugin tracks registered nodes and schedules pods to nodes when no specific node is requested.

## Overlay peer gossip

Overlay membership no longer fans out from the orchestrator to every node on every topology change. Instead:

- On `register_node`, the plugin returns an initial random sample of live peers (and any active tombstones) in `RegisterNodeResponse.seed_peers`, and makes **one** `gossip_peers` call to a single random existing peer announcing the newcomer. Knowledge spreads epidemically from there.
- On `unregister_node` or dead-node eviction, the plugin mints a tombstone in Redis and makes **one** `gossip_peers` call to a single random live peer with it.
- The old heartbeat-driven reconciliation (`push_overlay_peers_to_node`) is gone. DNS records and service routes are still reconciled on heartbeat for nodes that missed a broadcast; overlay state heals via SCOC-to-SCOC gossip anti-entropy.
- The `get_overlay_peers`, `add_overlay_peer`, and `remove_overlay_peer` RPCs have been removed from SCOP.

Tombstones are persisted as `ts:{name}` keys + a `tombstones` set in Redis. Expired tombstones (older than `--tombstone-ttl-secs`) are GC'd by a background task so the seed list stays bounded.

### New CLI flags

| Flag | Default | Description |
|------|---------|-------------|
| `--orchestrator-hostname` | *required* | Canonical, DNS-resolvable hostname the orchestrator uses to identify itself in `from_node` / `source` fields of outbound gossip. |
| `--tombstone-ttl-secs` | `3600` | How long tombstones are retained in Redis before GC. |
| `--seed-peer-count` | `5` | Maximum number of live peers included in `RegisterNodeResponse.seed_peers`. |

## Running

```sh
cargo run -p plugin_std_container -- \
  --bind 0.0.0.0:50053 \
  --orchestrator-hostname orchestrator.local \
  --rtp-bind tcp://0.0.0.0:50054 \
  --node-registry-hostname localhost \
  --cdb-hostnames localhost \
  --buildkit-addr tcp://localhost:1234 \
  --registry-url http://localhost:5000 \
  --ldb-hostname 127.0.0.1 \
  --cluster-cidr 10.42.0.0/16
```

### Enabling mTLS

Both directions of SCOP traffic (orchestrator RPCs from SCOC → plugin, conduit
RPCs from plugin → SCOC) can optionally run over mutual TLS. All three flags
are required together; omit all three to run plain gRPC.

```sh
cargo run -p plugin_std_container -- \
  ... \
  --tls-ca /etc/skyr/tls/ca.pem \
  --tls-cert /etc/skyr/tls/plugin.pem \
  --tls-key /etc/skyr/tls/plugin.key
```

The leaf certificate must carry both `serverAuth` and `clientAuth` Extended
Key Usages so one cert works for the orchestrator listener and for outbound
conduit connections. Use the same CA for the plugin and every SCOC node; see
[SCOC's README](../scoc/README.md#enabling-mtls) for an `openssl` recipe.

## Related Crates

- [IDs](../ids/) — parses deployment QIDs from resource namespaces
- [RTP](../rtp/) — the plugin protocol this implements
- [RTE](../rte/) — invokes this plugin to process transitions
- [SCOP](../scop/) — protocol for communicating with conduit nodes and serving orchestrator
- [SCOC](../scoc/) — conduit nodes that run containers
- [CDB](../cdb/) — reads repository contents for image builds
- [LDB](../ldb/) — writes build and deployment logs

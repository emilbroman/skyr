# Skyr Container Orchestrator Protocol (SCOP)

SCOP defines the gRPC protocol used between the [container plugin](../plugin_std_container/) and [conduit nodes](../scoc/) for container orchestration.

## Role in the Architecture

SCOP enables the container plugin to manage pods and containers on cluster nodes without direct access to the container runtime. It also provides the node registration protocol that conduit nodes use to join the cluster.

```
Container Plugin ← SCOP (Orchestrator) ← SCOC (register/heartbeat)
Container Plugin → SCOP (Conduit) → SCOC → containerd (CRI)
```

## Services

SCOP defines two separate gRPC services:

### Orchestrator Service

Served by the container plugin. Handles node lifecycle management.

| Method | Description |
|--------|-------------|
| `register_node` | Register a conduit node with its address and capacity; returns pod CIDR, service CIDR, and an initial seed of overlay peers (`seed_peers`). |
| `heartbeat` | Handle periodic heartbeats from registered nodes. |
| `unregister_node` | Remove a node from the cluster; the orchestrator mints a tombstone and gossips it to one random live peer. |

The orchestrator identifies itself in outbound gossip by a canonical, DNS-resolvable hostname passed via `--orchestrator-hostname` on the container plugin. This hostname appears as `from_node` on `GossipPeersRequest` and as the `source` on each `PeerEntry` the orchestrator mints.

### Conduit Service

Served by SCOC on each worker node. Handles pod and container operations plus overlay peer gossip.

| Category | Operations |
|----------|------------|
| Pod | `create_pod`, `remove_pod` |
| Container | `create_container`, `start_container`, `stop_container`, `remove_container` |
| Networking | `gossip_peers`, `open_port`, `close_port` |

#### Overlay peer gossip (`gossip_peers`)

Overlay membership is maintained by a gossip protocol rather than orchestrator fan-out. A single `GossipPeers` RPC carries both reactive deltas and an optional anti-entropy digest:

```
GossipPeersRequest {
  string       from_node;  // sender's canonical hostname
  PeerEntry[]  entries;    // add/remove deltas (may be empty)
  PeerDigest?  digest;     // for anti-entropy (may be absent)
}

PeerEntry {
  string node_name;          // canonical, DNS-resolvable identity
  string overlay_endpoint;   // VXLAN underlay host IP
  uint64 last_seen_micros;   // monotonic stamp minted by the orchestrator
  bool   tombstone;          // true = removal marker
  string source;             // last hop's hostname (or orchestrator's)
}
```

Ordering across the cluster is driven by `last_seen_micros`, which is minted only by the orchestrator (at register or eviction time) and preserved verbatim through gossip hops, so SCOCs do not need synchronized clocks. See [`crates/scoc/README.md`](../scoc/) for the merge rules and the SCOC-side state machine.

## Traits

- **`Orchestrator`** — implemented by the container plugin to handle node registration and heartbeats.
- **`Conduit`** — implemented by [SCOC](../scoc/) to handle pod, container, and networking commands.
- **`ConduitFactory`** — creates conduit instances for each incoming connection.

## Functions

| Function | Used by | Description |
|----------|---------|-------------|
| `serve()` | SCOC | Listen for plugin connections on a Conduit service |
| `dial()` | Container plugin | Connect to a conduit node |

## Transport

Supports TCP (`http://host:port`) and Unix socket (`unix:///path`) targets.

## Related Crates

- [SCOC](../scoc/) — implements the `Conduit` trait and serves SCOP
- [plugin_std_container](../plugin_std_container/) — implements the `Orchestrator` trait and dials conduit nodes via SCOP

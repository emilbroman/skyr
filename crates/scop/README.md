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
| `register_node` | Register a conduit node with its address and capacity |
| `heartbeat` | Handle periodic heartbeats from registered nodes |
| `unregister_node` | Remove a node from the cluster |

### Conduit Service

Served by SCOC on each worker node. Handles pod and container operations.

| Category | Operations |
|----------|------------|
| Pod | `create_pod`, `remove_pod` |
| Container | `create_container`, `start_container`, `stop_container`, `remove_container` |
| Networking | `add_overlay_peer`, `remove_overlay_peer`, `open_port`, `close_port` |

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

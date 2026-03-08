# Skyr Container Orchestrator Protocol (SCOP)

SCOP defines the bidirectional gRPC streaming protocol used between the [container plugin](../plugin_std_container/) and [conduit nodes](../scoc/).

## Role in the Architecture

SCOP enables the container plugin to manage pods and containers on cluster nodes without direct access to the container runtime. The plugin sends commands through SCOP sessions, and the conduit translates them to CRI calls.

```
Container Plugin → SCOP (gRPC stream) → SCOC → containerd (CRI)
```

## Protocol

The gRPC service `Conduit` provides a single bidirectional streaming RPC:

```
Session(stream PluginMessage) returns (stream ConduitMessage)
```

Request/response correlation uses unique request IDs.

### Commands

| Category | Operations |
|----------|------------|
| Pod | `run_pod_sandbox`, `stop_pod_sandbox`, `remove_pod_sandbox` |
| Container | `create_container`, `start_container`, `stop_container`, `remove_container` |

### Traits

- **`Conduit`** — implemented by [SCOC](../scoc/) to handle incoming commands.
- **`ConduitFactory`** — creates conduit instances for each incoming connection.

### Functions

| Function | Used by | Description |
|----------|---------|-------------|
| `serve()` | SCOC | Listen for plugin connections |
| `dial()` | Container plugin | Connect to a conduit node |

### Transport

Supports both TCP (`http://host:port`) and Unix socket (`unix:///path`) targets.

### Session

`dial()` returns a `Session` handle that the plugin uses to send commands and receive responses over the bidirectional stream.

## Related Crates

- [SCOC](../scoc/) — implements the `Conduit` trait and serves SCOP
- [plugin_std_container](../plugin_std_container/) — dials conduit nodes via SCOP

# Skyr Container Orchestrator Conduit (SCOC)

SCOC is a daemon that runs on cluster nodes and translates [SCOP](../scop/) commands into CRI (Container Runtime Interface) calls to containerd.

## Role in the Architecture

SCOC is the bridge between Skyr's container management layer and the actual container runtime. Each cluster node runs an SCOC instance that registers itself in the node registry (Redis) so the [container plugin](../plugin_std_container/) can discover and connect to it.

```
Container Plugin → SCOP → SCOC → containerd (CRI, Unix socket)
```

## How It Works

1. On startup, connects to containerd via Unix socket (default: `/run/containerd/containerd.sock`).
2. Registers its external address in the node registry (Redis).
3. Serves SCOP on a TCP port, accepting connections from the container plugin.
4. Translates incoming SCOP commands to CRI gRPC calls.
5. On shutdown, unregisters from the node registry.

## CRI Operations

| Category | Operations |
|----------|------------|
| Pod sandbox | `run_pod_sandbox`, `stop_pod_sandbox`, `remove_pod_sandbox` |
| Container | `create_container`, `start_container`, `stop_container`, `remove_container` |

## CLI

SCOC provides subcommands for testing CRI operations directly:

```sh
# Check containerd version
cargo run -p scoc -- version

# Pod operations
cargo run -p scoc -- pod run|stop|remove

# Container operations
cargo run -p scoc -- container create|start|stop|remove
```

### Daemon Mode

```sh
cargo run -p scoc -- daemon \
  --node-name node-1 \
  --bind 0.0.0.0:50060 \
  --external-address node-1:50060 \
  --node-registry-hostname localhost \
  --containerd-socket /run/containerd/containerd.sock
```

## Related Crates

- [SCOP](../scop/) — the protocol SCOC serves
- [plugin_std_container](../plugin_std_container/) — connects to SCOC via SCOP to manage containers

# Skyr Container Orchestrator Conduit (SCOC)

SCOC is a daemon that runs on cluster nodes and translates [SCOP](../scop/) commands into CRI (Container Runtime Interface) calls to containerd.

## Role in the Architecture

SCOC is the bridge between Skyr's container management layer and the actual container runtime. Each cluster node runs an SCOC instance that registers with the [container plugin](../plugin_std_container/) orchestrator so it can be discovered and receive pod/container commands.

```
Container Plugin → SCOP → SCOC → containerd (CRI, Unix socket)
                                → LDB (container log streaming)
```

## How It Works

1. Connects to containerd via Unix socket (default: `/run/containerd/containerd.sock`).
2. Connects to LDB for container log streaming.
3. Registers with the orchestrator (container plugin), reporting node capacity and requesting a pod subnet.
4. Receives pod CIDR assignment and cluster CIDR from the orchestrator.
5. Sets up the pod bridge network and VXLAN overlay for cross-node communication.
6. Spawns a heartbeat task (30-second intervals).
7. Serves the SCOP Conduit service on a TCP port.
8. On shutdown, tears down networking and unregisters from the orchestrator.

## Operations

| Category | Operations |
|----------|------------|
| Pod | `create_pod`, `remove_pod` |
| Container | `create_container`, `start_container`, `stop_container`, `remove_container` |
| Networking | `add_overlay_peer`, `remove_overlay_peer`, `open_port`, `close_port` |

## Networking

SCOC manages per-pod networking on each node:

- **Pod network**: Each pod gets a veth pair, bridge interface, and IP address allocated via per-node IPAM.
- **VXLAN overlay**: Cross-node pod communication uses a VXLAN overlay. Peers are added/removed as nodes join and leave the cluster.
- **Firewall**: Ingress ports are opened/closed per pod via `open_port`/`close_port`. Egress rules enforce an allow-list scoped to the cluster CIDR.

## Container Log Streaming

SCOC streams container logs to [LDB](../ldb/) using a per-container namespace format: `{environment_qid}::{pod_name}/{container_name}`. Each container gets a dedicated log publisher that follows the container's log file.

## CLI

SCOC provides subcommands for testing CRI operations directly:

```sh
# Check containerd version
cargo run -p scoc -- version

# Pod operations
cargo run -p scoc -- pod create|remove

# Container operations
cargo run -p scoc -- container create|start|stop|remove
```

### Daemon Mode

```sh
cargo run -p scoc -- daemon \
  --node-name node-1 \
  --bind 0.0.0.0:50054 \
  --conduit-address http://node-1:50054 \
  --orchestrator-address http://localhost:50053 \
  --containerd-socket /run/containerd/containerd.sock \
  --ldb-brokers 127.0.0.1:9092 \
  --cpu-millis 4000 \
  --memory-bytes 8589934592 \
  --max-pods 100 \
  --pod-netmask 24
```

## Related Crates

- [SCOP](../scop/) — the protocol SCOC serves
- [LDB](../ldb/) — container log streaming
- [plugin_std_container](../plugin_std_container/) — connects to SCOC via SCOP to manage containers

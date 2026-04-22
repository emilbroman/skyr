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
4. Receives pod CIDR assignment, cluster CIDR, and an initial seed of overlay peers in `RegisterNodeResponse`.
5. Sets up the pod bridge network and VXLAN overlay, seeding the FDB from the seed peer list.
6. Serves the SCOP Conduit service, including `gossip_peers` (see below).
7. Runs three background loops: periodic anti-entropy digest gossip, tombstone GC, and orchestrator heartbeats (30-second intervals).
8. On shutdown, tears down networking and unregisters from the orchestrator.

## Operations

| Category | Operations |
|----------|------------|
| Pod | `create_pod`, `remove_pod` |
| Container | `create_container`, `start_container`, `stop_container`, `remove_container` |
| Networking | `gossip_peers`, `open_port`, `close_port` |

## Overlay peer gossip

SCOC no longer learns about overlay peers by receiving per-event broadcasts from the orchestrator. Instead, each node keeps an in-memory `KnownPeers` table and exchanges deltas with other nodes via the SCOP `gossip_peers` RPC. The orchestrator only seeds the cluster: it hands a new node its initial peer list in `RegisterNodeResponse.seed_peers` and sends one gossip call to one live peer announcing the newcomer. Knowledge then spreads epidemically.

**State** (`crates/scoc/src/gossip.rs`): each entry is keyed by the peer's canonical hostname (`node_name`) and carries `overlay_endpoint`, `last_seen_micros`, `tombstone`, and `source`. Ordering is by `last_seen_micros`, which originates only at the orchestrator (at register or eviction time) and is preserved verbatim through gossip hops — no clock synchronization between SCOCs is required.

**Merge rules**:

- Unknown name, live entry: insert, install FDB entry.
- Stale timestamp (`<= existing`): drop.
- Live → tombstone: remove FDB entry, keep the tombstone until `--tombstone-ttl` expires.
- Tombstone → live (strictly newer stamp): install FDB entry (supersedes tombstone).
- Live → live with endpoint change: remove old FDB, install new.

Tombstones are minted **only** by the orchestrator — a SCOC never infers eviction from a peer becoming unreachable. This avoids split-brain removal.

**Fan-out**: when a merge produces net-new information, the node pushes the changed entries to `--gossip-fanout` random live peers (excluding self and the sender). Every `--gossip-interval-secs` the node additionally sends a compact digest to one random live peer for anti-entropy; any entries the peer returns in its delta are merged locally and participate in fan-out.

**CLI flags** (new):

| Flag | Default | Description |
|------|---------|-------------|
| `--gossip-fanout` | `3` | Random live peers pushed to on a reactive fan-out |
| `--gossip-interval-secs` | `30` | Period of the anti-entropy digest exchange |
| `--tombstone-ttl-secs` | `3600` | How long tombstones live locally before GC |

The old `get_overlay_peers`, `add_overlay_peer`, and `remove_overlay_peer` RPCs are gone — they have been replaced entirely by `gossip_peers` plus the seed-list field on `RegisterNodeResponse`.

## Networking

SCOC manages per-pod networking on each node:

- **Pod network**: Each pod gets a veth pair, bridge interface, and IP address allocated via per-node IPAM.
- **VXLAN overlay**: Cross-node pod communication uses a VXLAN overlay. Peers are added/removed as nodes join and leave the cluster.
- **Firewall**: Ingress ports are opened/closed per pod via `open_port`/`close_port`. Egress rules enforce an allow-list scoped to the cluster CIDR.

## iptables Architecture

SCOC configures iptables rules at two levels: the **host** (node-wide rules for bridging, NAT, and service routing) and **per-pod network namespaces** (firewall and egress control).

### Host: filter table

```
FORWARD chain:
  -i skyr0 -j ACCEPT                          # Pod egress: allow all traffic from bridge
  -o skyr0 -j ACCEPT                          # Pod ingress: allow all traffic to bridge
                                               #   (pods enforce their own INPUT firewalls)
```

The first rule allows pods to send traffic out. The second allows DNAT'd service traffic (and return traffic) to reach pods — this must be unconditional because after DNAT rewrites the destination to a pod IP, the packet appears as a NEW connection to the bridge.

### Host: nat table

```
POSTROUTING chain:
  -s <pod_cidr> ! -o skyr0 -j MASQUERADE      # Internet NAT for pod egress

PREROUTING chain:
  -j SKYR-SERVICES                             # Dispatch inbound traffic to service chains

OUTPUT chain:
  -j SKYR-SERVICES                             # Dispatch locally-originated traffic too
```

#### SKYR-SERVICES chain (nat)

Central dispatch for all Host.Port and InternetAddress routing. Contains one rule per exposed VIP:port:

```
SKYR-SERVICES:
  -d <vip> -p <proto> --dport <port> -j SKYR_SVC_<vip>_<port>_<proto>
  -d <alias_vip> -p <proto> --dport <port> -j SKYR_SVC_<target_vip>_<port>_<proto>
  ...
```

- **Host.Port VIPs** (service CIDR): dispatch rule jumps to the VIP's own per-service chain.
- **InternetAddress alias VIPs** (LAN IPs): dispatch rule jumps directly to the *target* Host.Port's per-service chain, achieving a single DNAT to the backend pod.

#### Per-service chains (nat)

Each `SKYR_SVC_<vip>_<port>_<proto>` chain contains backend rules:

```
SKYR_SVC_10_43_0_1_80_tcp:
  -m statistic --mode random --probability 0.5 -p tcp -j DNAT --to-destination 10.42.0.5:80
  -p tcp -j DNAT --to-destination 10.42.0.6:80
```

- **Pod backends**: terminal `DNAT` to the pod IP.
- **VIP backends** (Host.Port chaining): `-j SKYR_SVC_<backend_vip>_...` to jump to another service chain.
- Load balancing uses the `statistic` module with `--probability 1/N` for N backends.

### Per-pod network namespace: filter table

Each pod has its own iptables rules inside its network namespace:

```
INPUT chain (default DROP):
  -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT   # Return traffic
  -i lo -j ACCEPT                                         # Loopback
  -p <proto> --dport <port> -j ACCEPT                     # Explicitly opened ports (Pod.Port)

OUTPUT chain (default ACCEPT):
  -o lo -j ACCEPT                                         # Loopback
  -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT    # Return traffic
  -j SKYR-EGRESS                                          # Attachment-managed rules
  -d <cluster_cidr> -j DROP                               # Block cluster-internal
  -d <service_cidr> -j DROP                               # Block Host VIPs
                                                          # (internet falls through to ACCEPT)

SKYR-EGRESS chain:
  -d <addr> -p <proto> --dport <port> -j ACCEPT           # Per-attachment egress allows
```

Pods are deny-all ingress by default. Ports are opened explicitly via `open_port` (for Pod.Port resources). Egress blocks cluster-internal and service VIP traffic by default — attachments can punch holes via the SKYR-EGRESS chain.

### InternetAddress: L2 + L3

InternetAddress resources combine two mechanisms:

1. **L2 (ARP)**: The LAN VIP is added to the node's primary interface (`ip addr add <vip>/32 dev <iface>`) with `arp_notify` enabled for gratuitous ARP.
2. **L3 (routing)**: A dispatch rule in SKYR-SERVICES routes `<alias_vip>:port` directly to the target Host.Port's per-service chain — single DNAT, no double-DNAT.

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
  --pod-netmask 24 \
  --gossip-fanout 3 \
  --gossip-interval-secs 30 \
  --tombstone-ttl-secs 3600
```

### Enabling mTLS

SCOC and the container plugin can optionally authenticate each other with
mutual TLS. All three flags are required together; omit all three to run
plain gRPC.

```sh
cargo run -p scoc -- daemon \
  ... \
  --tls-ca /etc/scoc/tls/ca.pem \
  --tls-cert /etc/scoc/tls/node.pem \
  --tls-key /etc/scoc/tls/node.key
```

The leaf certificate must carry both `serverAuth` and `clientAuth` Extended
Key Usages because SCOC acts as a gRPC server (for conduit RPCs from the
plugin) and as a gRPC client (for `register_node`/`heartbeat`/`unregister_node`
calls to the orchestrator). Use the same CA on both sides and issue a leaf
cert per node with the node hostname in a SAN that matches the conduit
address passed via `--conduit-address`.

Example issuance with `openssl` (simplified — production deployments should
use an automated PKI):

```sh
# 1. Self-signed CA
openssl req -x509 -newkey rsa:4096 -nodes -days 3650 \
  -subj "/CN=Skyr SCOP CA" -keyout ca.key -out ca.pem

# 2. Leaf key + CSR
openssl req -newkey rsa:4096 -nodes \
  -subj "/CN=scoc-1" \
  -addext "subjectAltName=DNS:scoc-1" \
  -addext "extendedKeyUsage=serverAuth,clientAuth" \
  -keyout node.key -out node.csr

# 3. Sign with CA, preserving EKUs
openssl x509 -req -days 365 -in node.csr -CA ca.pem -CAkey ca.key \
  -CAcreateserial -out node.pem \
  -copy_extensions copyall
```

The container plugin takes the same three flags (`--tls-ca`, `--tls-cert`,
`--tls-key`) — see `plugin_std_container`'s README.

In container deployments the matching env vars `SCOC_TLS_CA`, `SCOC_TLS_CERT`,
`SCOC_TLS_KEY` are forwarded by `dev/scoc-entrypoint.sh`.

## Related Crates

- [SCOP](../scop/) — the protocol SCOC serves
- [LDB](../ldb/) — container log streaming
- [plugin_std_container](../plugin_std_container/) — connects to SCOC via SCOP to manage containers

//! Pod networking: bridge setup, veth plumbing, IPAM, and firewall rules.
//!
//! SCOC manages per-pod network namespaces with deny-all ingress by default.
//! Each node has a Linux bridge (`skyr0`) with a gateway IP, and each pod gets
//! a veth pair connecting it to the bridge. Outbound internet access is provided
//! via iptables MASQUERADE on the host.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::net::Ipv4Addr;
use std::process::Command;

use anyhow::{Context, Result, bail};
use ipnet::Ipv4Net;
use tracing::{debug, info, warn};

/// Bridge interface name used for pod networking.
const BRIDGE_NAME: &str = "skyr0";

/// VXLAN interface name for overlay networking.
const VXLAN_NAME: &str = "vxlan1";

/// VXLAN Network Identifier.
const VXLAN_VNI: u32 = 42;

/// VXLAN UDP destination port.
const VXLAN_PORT: u16 = 4789;

// ============================================================================
// IPAM — per-node IP address management
// ============================================================================

/// Simple in-memory IPAM for a single node's subnet.
///
/// Allocates IPs sequentially from .2 upward (.1 is the gateway on the bridge).
/// Released IPs are recycled.
#[allow(dead_code)]
pub struct Ipam {
    subnet: Ipv4Net,
    gateway: Ipv4Addr,
    /// IPs currently in use, keyed by pod ID.
    allocated: HashMap<String, Ipv4Addr>,
    /// IPs that were released and can be reused.
    free_pool: Vec<Ipv4Addr>,
    /// Next IP to allocate (when free_pool is empty).
    next_offset: u32,
}

impl Ipam {
    /// Create a new IPAM for the given subnet.
    ///
    /// The gateway is the first usable host address (.1), and pod IPs start at .2.
    pub fn new(subnet: Ipv4Net) -> Self {
        let network = subnet.network();
        let gateway = Ipv4Addr::from(u32::from(network) + 1);
        Self {
            subnet,
            gateway,
            allocated: HashMap::new(),
            free_pool: Vec::new(),
            next_offset: 2, // .0 is network, .1 is gateway, pods start at .2
        }
    }

    /// The gateway IP for this subnet (assigned to the bridge).
    #[allow(dead_code)]
    pub fn gateway(&self) -> Ipv4Addr {
        self.gateway
    }

    /// The subnet managed by this IPAM.
    #[allow(dead_code)]
    pub fn subnet(&self) -> &Ipv4Net {
        &self.subnet
    }

    /// Allocate an IP for a pod. Returns the assigned address.
    pub fn allocate(&mut self, pod_id: &str) -> Result<Ipv4Addr> {
        if let Some(&existing) = self.allocated.get(pod_id) {
            return Ok(existing);
        }

        let ip = if let Some(ip) = self.free_pool.pop() {
            ip
        } else {
            let network_u32 = u32::from(self.subnet.network());
            let candidate = network_u32 + self.next_offset;
            let broadcast = u32::from(self.subnet.broadcast());
            if candidate >= broadcast {
                bail!(
                    "IPAM exhausted: no more addresses in subnet {}",
                    self.subnet
                );
            }
            self.next_offset += 1;
            Ipv4Addr::from(candidate)
        };

        self.allocated.insert(pod_id.to_string(), ip);
        info!(pod_id = %pod_id, ip = %ip, "allocated pod IP");
        Ok(ip)
    }

    /// Release a pod's IP back to the pool.
    pub fn release(&mut self, pod_id: &str) {
        if let Some(ip) = self.allocated.remove(pod_id) {
            info!(pod_id = %pod_id, ip = %ip, "released pod IP");
            self.free_pool.push(ip);
        }
    }
}

// ============================================================================
// Bridge lifecycle
// ============================================================================

/// Set up the node bridge for pod networking.
///
/// Creates the `skyr0` bridge, assigns the gateway IP, enables IP forwarding,
/// and installs iptables rules for NAT and inter-pod isolation.
pub fn setup_bridge(subnet: &Ipv4Net) -> Result<()> {
    let gateway = Ipv4Addr::from(u32::from(subnet.network()) + 1);
    let gateway_cidr = format!("{}/{}", gateway, subnet.prefix_len());

    // Clean up any stale bridge from a previous run
    if bridge_exists()? {
        warn!("stale bridge {} found, removing before setup", BRIDGE_NAME);
        let _ = teardown_bridge(subnet);
    }

    info!(bridge = BRIDGE_NAME, gateway = %gateway_cidr, "setting up pod bridge");

    // Create bridge
    run_cmd("ip", &["link", "add", BRIDGE_NAME, "type", "bridge"])
        .context("failed to create bridge")?;

    // Assign gateway IP
    run_cmd("ip", &["addr", "add", &gateway_cidr, "dev", BRIDGE_NAME])
        .context("failed to assign gateway IP to bridge")?;

    // Bring bridge up
    run_cmd("ip", &["link", "set", BRIDGE_NAME, "up"]).context("failed to bring bridge up")?;

    // Enable IP forwarding
    std::fs::write("/proc/sys/net/ipv4/ip_forward", "1")
        .context("failed to enable IP forwarding")?;

    // NAT: masquerade pod traffic going to the internet
    run_cmd(
        "iptables",
        &[
            "-t",
            "nat",
            "-A",
            "POSTROUTING",
            "-s",
            &subnet.to_string(),
            "!",
            "-o",
            BRIDGE_NAME,
            "-j",
            "MASQUERADE",
        ],
    )
    .context("failed to add MASQUERADE rule")?;

    // Allow forwarding from bridge
    run_cmd(
        "iptables",
        &["-A", "FORWARD", "-i", BRIDGE_NAME, "-j", "ACCEPT"],
    )
    .context("failed to add FORWARD accept rule for bridge")?;

    // Allow forwarding to bridge (for return traffic)
    run_cmd(
        "iptables",
        &[
            "-A",
            "FORWARD",
            "-o",
            BRIDGE_NAME,
            "-m",
            "conntrack",
            "--ctstate",
            "RELATED,ESTABLISHED",
            "-j",
            "ACCEPT",
        ],
    )
    .context("failed to add FORWARD return traffic rule")?;

    info!("pod bridge setup complete");
    Ok(())
}

/// Tear down the node bridge and remove associated iptables rules.
pub fn teardown_bridge(subnet: &Ipv4Net) -> Result<()> {
    info!(bridge = BRIDGE_NAME, "tearing down pod bridge");

    // Remove iptables rules (ignore errors — rules may not exist)
    let _ = run_cmd(
        "iptables",
        &[
            "-t",
            "nat",
            "-D",
            "POSTROUTING",
            "-s",
            &subnet.to_string(),
            "!",
            "-o",
            BRIDGE_NAME,
            "-j",
            "MASQUERADE",
        ],
    );
    let _ = run_cmd(
        "iptables",
        &["-D", "FORWARD", "-i", BRIDGE_NAME, "-j", "ACCEPT"],
    );
    let _ = run_cmd(
        "iptables",
        &[
            "-D",
            "FORWARD",
            "-o",
            BRIDGE_NAME,
            "-m",
            "conntrack",
            "--ctstate",
            "RELATED,ESTABLISHED",
            "-j",
            "ACCEPT",
        ],
    );

    // Delete bridge (this also removes attached veth host-side interfaces)
    let _ = run_cmd("ip", &["link", "del", BRIDGE_NAME]);

    info!("pod bridge teardown complete");
    Ok(())
}

fn bridge_exists() -> Result<bool> {
    let output = Command::new("ip")
        .args(["link", "show", BRIDGE_NAME])
        .output()
        .context("failed to check bridge existence")?;
    Ok(output.status.success())
}

// ============================================================================
// Pod network setup/teardown
// ============================================================================

/// Set up networking for a pod.
///
/// Creates a veth pair, attaches the host end to the bridge, moves the pod end
/// into the pod's network namespace, assigns an IP, configures routes, and
/// installs deny-all ingress firewall rules. If an allow list and cluster CIDR
/// are provided, egress rules are also configured to restrict cluster-internal
/// traffic to only the allowed destinations.
pub fn setup_pod_network(
    pod_id: &str,
    ip: Ipv4Addr,
    subnet: &Ipv4Net,
    netns_path: &str,
    allowed_destinations: &[scop::AllowedDestination],
    cluster_cidr: Option<&str>,
    service_cidr: Option<&str>,
) -> Result<()> {
    let gateway = Ipv4Addr::from(u32::from(subnet.network()) + 1);
    let ip_cidr = format!("{}/{}", ip, subnet.prefix_len());
    let host_veth = veth_host_name(pod_id);
    // Use "skyr0" instead of "eth0" to avoid conflicts with CNI-created interfaces
    let pod_veth = "skyr0";

    info!(
        pod_id = %pod_id,
        ip = %ip_cidr,
        host_veth = %host_veth,
        netns = %netns_path,
        "setting up pod network"
    );

    // Create veth pair with pod end directly in the target namespace.
    // We use "ip link add ... netns <path>" which creates the peer in the target netns.
    // This avoids issues with moving interfaces between namespaces in nested containers.
    run_cmd(
        "ip",
        &[
            "link", "add", &host_veth, "type", "veth", "peer", "name", pod_veth, "netns",
            netns_path,
        ],
    )
    .with_context(|| format!("failed to create veth pair for pod {pod_id}"))?;

    // Attach host end to bridge
    run_cmd("ip", &["link", "set", &host_veth, "master", BRIDGE_NAME])
        .with_context(|| format!("failed to attach {host_veth} to bridge"))?;

    // Bring host end up
    run_cmd("ip", &["link", "set", &host_veth, "up"])
        .with_context(|| format!("failed to bring {host_veth} up"))?;

    // Configure pod-side networking inside the namespace
    nsenter_run(
        netns_path,
        "ip",
        &["addr", "add", &ip_cidr, "dev", pod_veth],
    )
    .context("failed to assign IP in pod netns")?;

    nsenter_run(netns_path, "ip", &["link", "set", pod_veth, "up"])
        .context("failed to bring eth0 up in pod netns")?;

    nsenter_run(netns_path, "ip", &["link", "set", "lo", "up"])
        .context("failed to bring loopback up in pod netns")?;

    nsenter_run(
        netns_path,
        "ip",
        &["route", "add", "default", "via", &gateway.to_string()],
    )
    .context("failed to add default route in pod netns")?;

    // Apply deny-all ingress firewall
    setup_pod_firewall(netns_path)
        .with_context(|| format!("failed to set up firewall for pod {pod_id}"))?;

    // Apply egress allow-list rules (restricts cluster-internal traffic)
    if let Some(cidr) = cluster_cidr {
        setup_pod_egress_rules(netns_path, allowed_destinations, cidr, service_cidr)
            .with_context(|| format!("failed to set up egress rules for pod {pod_id}"))?;
    }

    info!(pod_id = %pod_id, "pod network setup complete");
    Ok(())
}

/// Tear down networking for a pod.
///
/// Deletes the host-side veth interface (the pod-side is cleaned up when
/// the network namespace is destroyed by the CRI).
pub fn teardown_pod_network(pod_id: &str) -> Result<()> {
    let host_veth = veth_host_name(pod_id);
    info!(pod_id = %pod_id, host_veth = %host_veth, "tearing down pod network");

    // Delete the host-side veth; this automatically removes the peer
    let _ = run_cmd("ip", &["link", "del", &host_veth]);
    Ok(())
}

/// Generate a deterministic host-side veth name from a pod ID.
///
/// Linux interface names are limited to 15 characters. We use "veth" (4 chars)
/// plus 11 hex chars from a simple hash of the pod ID.
fn veth_host_name(pod_id: &str) -> String {
    let hash = simple_hash(pod_id);
    let mut name = String::with_capacity(15);
    name.push_str("veth");
    write!(&mut name, "{:011x}", hash & 0x00FFFFFFFFFFF).unwrap();
    name
}

/// Simple non-cryptographic hash for generating short unique interface names.
fn simple_hash(s: &str) -> u64 {
    // FNV-1a 64-bit
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in s.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

// ============================================================================
// Firewall
// ============================================================================

/// Set up deny-all ingress firewall inside a pod's network namespace.
///
/// Rules:
/// - INPUT default policy: DROP
/// - Allow ESTABLISHED,RELATED (responses to outgoing traffic)
/// - Allow loopback
/// - OUTPUT default policy: ACCEPT (internet access)
/// - FORWARD default policy: DROP
fn setup_pod_firewall(netns_path: &str) -> Result<()> {
    debug!(netns = %netns_path, "configuring pod firewall");

    // Default policies
    nsenter_run(netns_path, "iptables", &["-P", "INPUT", "DROP"])?;
    nsenter_run(netns_path, "iptables", &["-P", "FORWARD", "DROP"])?;
    nsenter_run(netns_path, "iptables", &["-P", "OUTPUT", "ACCEPT"])?;

    // Allow established/related connections (return traffic)
    nsenter_run(
        netns_path,
        "iptables",
        &[
            "-A",
            "INPUT",
            "-m",
            "conntrack",
            "--ctstate",
            "ESTABLISHED,RELATED",
            "-j",
            "ACCEPT",
        ],
    )?;

    // Allow loopback
    nsenter_run(
        netns_path,
        "iptables",
        &["-A", "INPUT", "-i", "lo", "-j", "ACCEPT"],
    )?;

    Ok(())
}

/// Configure egress rules for a pod based on its allow list.
///
/// By default pods can reach the internet but not other cluster-internal
/// destinations. The allow list grants access to specific address:port
/// pairs within the cluster.
///
/// Rules (appended to the OUTPUT chain, which defaults to ACCEPT):
/// 1. Allow loopback output
/// 2. Allow established/related outbound connections
/// 3. For each allowed destination: allow traffic to that address:port/protocol
/// 4. Drop all other traffic to the cluster CIDR (blocks cluster-internal)
/// 5. Everything else (internet) falls through to the ACCEPT policy
fn setup_pod_egress_rules(
    netns_path: &str,
    allowed: &[scop::AllowedDestination],
    cluster_cidr: &str,
    service_cidr: Option<&str>,
) -> Result<()> {
    debug!(
        netns = %netns_path,
        num_allowed = %allowed.len(),
        cluster_cidr = %cluster_cidr,
        "configuring pod egress rules"
    );

    // Allow loopback output (always needed)
    nsenter_run(
        netns_path,
        "iptables",
        &["-A", "OUTPUT", "-o", "lo", "-j", "ACCEPT"],
    )?;

    // Allow established/related outbound (for return traffic on accepted connections)
    nsenter_run(
        netns_path,
        "iptables",
        &[
            "-A",
            "OUTPUT",
            "-m",
            "conntrack",
            "--ctstate",
            "ESTABLISHED,RELATED",
            "-j",
            "ACCEPT",
        ],
    )?;

    // Allow traffic to each destination in the allow list.
    // Destinations can be either pod IPs (for Pod.Port) or VIPs (for Host.Port).
    for dest in allowed {
        let port_str = dest.port.to_string();
        nsenter_run(
            netns_path,
            "iptables",
            &[
                "-A",
                "OUTPUT",
                "-d",
                &dest.address,
                "-p",
                &dest.protocol,
                "--dport",
                &port_str,
                "-j",
                "ACCEPT",
            ],
        )?;

        debug!(
            netns = %netns_path,
            dest_address = %dest.address,
            dest_port = %dest.port,
            dest_protocol = %dest.protocol,
            "added egress allow rule"
        );
    }

    // Drop all other traffic to cluster-internal IPs (pod CIDR)
    nsenter_run(
        netns_path,
        "iptables",
        &["-A", "OUTPUT", "-d", cluster_cidr, "-j", "DROP"],
    )?;

    // Drop all other traffic to the service CIDR (Host VIPs) unless explicitly allowed
    if let Some(svc_cidr) = service_cidr {
        nsenter_run(
            netns_path,
            "iptables",
            &["-A", "OUTPUT", "-d", svc_cidr, "-j", "DROP"],
        )?;
    }

    // Everything else (internet) falls through to OUTPUT ACCEPT policy

    info!(
        netns = %netns_path,
        num_allowed = %allowed.len(),
        "pod egress rules configured"
    );

    Ok(())
}

// ============================================================================
// VXLAN overlay
// ============================================================================

/// Set up a VXLAN interface and attach it to the pod bridge.
///
/// This enables cross-node pod communication. The VXLAN uses the bridge's
/// L2 learning for MAC-to-VTEP resolution after BUM traffic is flooded
/// to all peers via FDB entries.
pub fn setup_vxlan(local_ip: &str) -> Result<()> {
    let vni = VXLAN_VNI.to_string();
    let port = VXLAN_PORT.to_string();

    info!(
        vxlan = VXLAN_NAME,
        local_ip = %local_ip,
        vni = %vni,
        "setting up VXLAN overlay"
    );

    run_cmd(
        "ip",
        &[
            "link",
            "add",
            VXLAN_NAME,
            "type",
            "vxlan",
            "id",
            &vni,
            "dstport",
            &port,
            "local",
            local_ip,
            "nolearning",
        ],
    )
    .context("failed to create VXLAN interface")?;

    run_cmd("ip", &["link", "set", VXLAN_NAME, "master", BRIDGE_NAME])
        .context("failed to attach VXLAN to bridge")?;

    run_cmd("ip", &["link", "set", VXLAN_NAME, "up"])
        .context("failed to bring VXLAN interface up")?;

    info!("VXLAN overlay setup complete");
    Ok(())
}

/// Tear down the VXLAN interface.
pub fn teardown_vxlan() -> Result<()> {
    info!(vxlan = VXLAN_NAME, "tearing down VXLAN overlay");
    let _ = run_cmd("ip", &["link", "del", VXLAN_NAME]);
    Ok(())
}

/// Add a VXLAN overlay peer.
///
/// Adds an FDB entry that directs BUM (broadcast, unknown-unicast, multicast)
/// traffic to the peer's underlay IP. This enables ARP resolution across nodes.
pub fn add_overlay_peer(peer_ip: &str) -> Result<()> {
    info!(peer_ip = %peer_ip, "adding overlay peer");
    run_cmd(
        "bridge",
        &[
            "fdb",
            "append",
            "00:00:00:00:00:00",
            "dev",
            VXLAN_NAME,
            "dst",
            peer_ip,
        ],
    )
    .with_context(|| format!("failed to add overlay peer {peer_ip}"))
}

/// Remove a VXLAN overlay peer.
pub fn remove_overlay_peer(peer_ip: &str) -> Result<()> {
    info!(peer_ip = %peer_ip, "removing overlay peer");
    run_cmd(
        "bridge",
        &[
            "fdb",
            "del",
            "00:00:00:00:00:00",
            "dev",
            VXLAN_NAME,
            "dst",
            peer_ip,
        ],
    )
    .with_context(|| format!("failed to remove overlay peer {peer_ip}"))
}

// ============================================================================
// Ingress port management
// ============================================================================

/// Open an ingress firewall port on a pod.
///
/// Appends an iptables INPUT ACCEPT rule for the specified port/protocol
/// in the pod's network namespace.
pub fn open_port(netns_path: &str, port: i32, protocol: &str) -> Result<()> {
    let port_str = port.to_string();
    info!(
        netns = %netns_path,
        port = %port,
        protocol = %protocol,
        "opening ingress port"
    );
    nsenter_run(
        netns_path,
        "iptables",
        &[
            "-A", "INPUT", "-p", protocol, "--dport", &port_str, "-j", "ACCEPT",
        ],
    )
    .with_context(|| format!("failed to open port {port}/{protocol}"))
}

/// Close an ingress firewall port on a pod.
///
/// Deletes the matching iptables INPUT ACCEPT rule from the pod's
/// network namespace.
pub fn close_port(netns_path: &str, port: i32, protocol: &str) -> Result<()> {
    let port_str = port.to_string();
    info!(
        netns = %netns_path,
        port = %port,
        protocol = %protocol,
        "closing ingress port"
    );
    nsenter_run(
        netns_path,
        "iptables",
        &[
            "-D", "INPUT", "-p", protocol, "--dport", &port_str, "-j", "ACCEPT",
        ],
    )
    .with_context(|| format!("failed to close port {port}/{protocol}"))
}

// ============================================================================
// Service route management (DNAT for Host.Port VIPs)
// ============================================================================

/// Custom iptables chain name for service routes (DNAT rules).
const SERVICES_CHAIN: &str = "SKYR-SERVICES";

/// Set up the services DNAT chain in the nat table.
///
/// Creates a custom chain and hooks it into PREROUTING and OUTPUT so that
/// both incoming bridge traffic (from pods) and locally-originated traffic
/// are subject to DNAT rules.
pub fn setup_services_chain() -> Result<()> {
    info!("setting up SKYR-SERVICES iptables chain");

    // Create the custom chain in the nat table
    let _ = run_cmd("iptables", &["-t", "nat", "-N", SERVICES_CHAIN]);

    // Hook into PREROUTING (traffic entering the node, e.g. from pods via bridge)
    let _ = run_cmd(
        "iptables",
        &["-t", "nat", "-C", "PREROUTING", "-j", SERVICES_CHAIN],
    )
    .or_else(|_| {
        run_cmd(
            "iptables",
            &["-t", "nat", "-A", "PREROUTING", "-j", SERVICES_CHAIN],
        )
    });

    // Hook into OUTPUT (traffic generated locally on the node)
    let _ = run_cmd(
        "iptables",
        &["-t", "nat", "-C", "OUTPUT", "-j", SERVICES_CHAIN],
    )
    .or_else(|_| {
        run_cmd(
            "iptables",
            &["-t", "nat", "-A", "OUTPUT", "-j", SERVICES_CHAIN],
        )
    });

    info!("SKYR-SERVICES chain setup complete");
    Ok(())
}

/// Tear down the services DNAT chain.
pub fn teardown_services_chain() -> Result<()> {
    info!("tearing down SKYR-SERVICES iptables chain");
    let _ = run_cmd(
        "iptables",
        &["-t", "nat", "-D", "PREROUTING", "-j", SERVICES_CHAIN],
    );
    let _ = run_cmd(
        "iptables",
        &["-t", "nat", "-D", "OUTPUT", "-j", SERVICES_CHAIN],
    );
    let _ = run_cmd("iptables", &["-t", "nat", "-F", SERVICES_CHAIN]);
    let _ = run_cmd("iptables", &["-t", "nat", "-X", SERVICES_CHAIN]);
    Ok(())
}

/// Add a DNAT service route: VIP:port → load-balanced backends.
///
/// For multiple backends, uses the iptables `statistic` module with
/// `--probability` for round-robin load balancing.
pub fn add_service_route(
    vip: &str,
    port: i32,
    protocol: &str,
    backends: &[scop::ServiceBackend],
) -> Result<()> {
    if backends.is_empty() {
        warn!(vip = %vip, port = %port, "no backends for service route, skipping");
        return Ok(());
    }

    let port_str = port.to_string();
    let num_backends = backends.len();

    info!(
        vip = %vip,
        port = %port,
        protocol = %protocol,
        num_backends = %num_backends,
        "adding service route"
    );

    for (i, backend) in backends.iter().enumerate() {
        let dest = format!("{}:{}", backend.address, backend.port);

        // For load balancing: use statistic --probability for all but the last backend.
        // Last backend catches everything remaining.
        if i < num_backends - 1 {
            let remaining = num_backends - i;
            let probability = format!("{:.10}", 1.0 / remaining as f64);
            run_cmd(
                "iptables",
                &[
                    "-t",
                    "nat",
                    "-A",
                    SERVICES_CHAIN,
                    "-d",
                    vip,
                    "-p",
                    protocol,
                    "--dport",
                    &port_str,
                    "-m",
                    "statistic",
                    "--mode",
                    "random",
                    "--probability",
                    &probability,
                    "-j",
                    "DNAT",
                    "--to-destination",
                    &dest,
                ],
            )
            .with_context(|| format!("failed to add service route {vip}:{port} → {dest}"))?;
        } else {
            // Last backend: no probability filter, catches all remaining traffic
            run_cmd(
                "iptables",
                &[
                    "-t",
                    "nat",
                    "-A",
                    SERVICES_CHAIN,
                    "-d",
                    vip,
                    "-p",
                    protocol,
                    "--dport",
                    &port_str,
                    "-j",
                    "DNAT",
                    "--to-destination",
                    &dest,
                ],
            )
            .with_context(|| format!("failed to add service route {vip}:{port} → {dest}"))?;
        }
    }

    info!(vip = %vip, port = %port, "service route added");
    Ok(())
}

/// Remove a DNAT service route for a VIP:port.
///
/// Removes all DNAT rules in the SKYR-SERVICES chain matching this VIP:port.
pub fn remove_service_route(vip: &str, port: i32, protocol: &str) -> Result<()> {
    let port_str = port.to_string();

    info!(
        vip = %vip,
        port = %port,
        protocol = %protocol,
        "removing service route"
    );

    // List all rules in the chain and delete matching ones.
    // We loop because iptables -D only removes one rule at a time,
    // and there may be multiple backends.
    loop {
        let output = std::process::Command::new("iptables")
            .args(["-t", "nat", "-S", SERVICES_CHAIN])
            .output()
            .context("failed to list service rules")?;

        let rules = String::from_utf8_lossy(&output.stdout);
        let rule_to_delete = rules.lines().find(|line| {
            line.contains(&format!("-d {}", vip))
                && line.contains(&format!("--dport {}", port_str))
                && line.contains(&format!("-p {}", protocol))
                && line.contains("-j DNAT")
        });

        if let Some(rule) = rule_to_delete {
            // Convert -A to -D for deletion
            let delete_rule = rule.replace("-A ", "-D ");
            let args: Vec<&str> = std::iter::once("-t")
                .chain(std::iter::once("nat"))
                .chain(delete_rule.split_whitespace())
                .collect();
            let _ = run_cmd("iptables", &args);
        } else {
            break;
        }
    }

    info!(vip = %vip, port = %port, "service route removed");
    Ok(())
}

/// Configure the service CIDR for FORWARD rules.
///
/// Allows forwarding of traffic destined to the service CIDR through the bridge,
/// so DNAT'd packets can reach their backend pods.
pub fn configure_service_cidr_forwarding(service_cidr: &str) -> Result<()> {
    info!(
        service_cidr = %service_cidr,
        "configuring service CIDR forwarding"
    );

    // Allow forwarding from bridge to service CIDR destinations
    // (after DNAT rewrites the destination, traffic needs to be forwarded)
    run_cmd(
        "iptables",
        &[
            "-A",
            "FORWARD",
            "-o",
            BRIDGE_NAME,
            "-d",
            service_cidr,
            "-j",
            "ACCEPT",
        ],
    )
    .context("failed to add FORWARD rule for service CIDR")?;

    Ok(())
}

// ============================================================================
// DNS
// ============================================================================

/// Read the host's DNS configuration for forwarding to pods.
///
/// Returns a list of nameserver IPs found in /etc/resolv.conf.
pub fn host_nameservers() -> Vec<String> {
    let content = match std::fs::read_to_string("/etc/resolv.conf") {
        Ok(c) => c,
        Err(_) => return vec!["8.8.8.8".to_string()],
    };

    let servers: Vec<String> = content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with("nameserver") {
                line.split_whitespace().nth(1).map(String::from)
            } else {
                None
            }
        })
        .collect();

    if servers.is_empty() {
        vec!["8.8.8.8".to_string()]
    } else {
        servers
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Run a command and return an error if it fails.
fn run_cmd(program: &str, args: &[&str]) -> Result<()> {
    debug!(cmd = %program, args = ?args, "running command");
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("failed to execute {program}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "{} {} failed (exit {}): {}",
            program,
            args.join(" "),
            output.status,
            stderr.trim()
        );
    }
    Ok(())
}

/// Run a command inside a network namespace using nsenter.
fn nsenter_run(netns_path: &str, program: &str, args: &[&str]) -> Result<()> {
    // Use --net=PATH format (util-linux nsenter requires = for file paths)
    let net_arg = format!("--net={}", netns_path);
    let mut nsenter_args = vec![net_arg.as_str(), "--", program];
    nsenter_args.extend_from_slice(args);
    run_cmd("nsenter", &nsenter_args)
}

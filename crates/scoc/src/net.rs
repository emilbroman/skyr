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

// ============================================================================
// Input validation helpers
// ============================================================================

/// Validate that a string is a valid IPv4 address.
fn validate_ipv4(s: &str) -> Result<Ipv4Addr> {
    s.parse::<Ipv4Addr>()
        .with_context(|| format!("invalid IPv4 address: {s:?}"))
}

/// Validate that a string is a valid CIDR notation (e.g., "10.0.0.0/24").
fn validate_cidr(s: &str) -> Result<Ipv4Net> {
    s.parse::<Ipv4Net>()
        .with_context(|| format!("invalid CIDR notation: {s:?}"))
}

/// Validate that a port number is in the valid range (1-65535).
fn validate_port(port: i32) -> Result<u16> {
    if !(1..=65535).contains(&port) {
        bail!("port number out of range (1-65535): {port}");
    }
    Ok(port as u16)
}

/// Validate that a protocol string is one of the allowed values.
fn validate_protocol(protocol: &str) -> Result<&str> {
    match protocol {
        "tcp" | "udp" => Ok(protocol),
        _ => bail!("invalid protocol (expected \"tcp\" or \"udp\"): {protocol:?}"),
    }
}

/// Validate that a network namespace path looks legitimate.
///
/// Must be an absolute path under `/proc/` to prevent path traversal.
fn validate_netns_path(path: &str) -> Result<()> {
    if !path.starts_with("/proc/") || path.contains("..") {
        bail!("invalid network namespace path (must be under /proc/ with no ..): {path:?}");
    }
    Ok(())
}

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
pub(crate) struct Ipam {
    subnet: Ipv4Net,
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
    pub(crate) fn new(subnet: Ipv4Net) -> Self {
        Self {
            subnet,
            allocated: HashMap::new(),
            free_pool: Vec::new(),
            next_offset: 2, // .0 is network, .1 is gateway, pods start at .2
        }
    }

    /// Allocate an IP for a pod. Returns the assigned address.
    pub(crate) fn allocate(&mut self, pod_id: &str) -> Result<Ipv4Addr> {
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
    pub(crate) fn release(&mut self, pod_id: &str) {
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
pub(crate) fn setup_bridge(subnet: &Ipv4Net) -> Result<()> {
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

    // Allow forwarding to bridge (for DNAT'd service traffic and return traffic).
    // Pods have their own INPUT firewalls (DROP by default + explicit port opens),
    // so we don't need to restrict at the FORWARD level. This is necessary because
    // after DNAT rewrites the destination to a pod IP, the packet needs to be
    // forwarded through the bridge as a NEW connection.
    run_cmd(
        "iptables",
        &["-A", "FORWARD", "-o", BRIDGE_NAME, "-j", "ACCEPT"],
    )
    .context("failed to add FORWARD rule for bridge")?;

    info!("pod bridge setup complete");
    Ok(())
}

/// Tear down the node bridge and remove associated iptables rules.
pub(crate) fn teardown_bridge(subnet: &Ipv4Net) -> Result<()> {
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
        &["-D", "FORWARD", "-o", BRIDGE_NAME, "-j", "ACCEPT"],
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
/// installs deny-all ingress firewall rules. Egress rules restrict cluster-internal
/// traffic by default; use `open_egress_port` to allow specific destinations.
pub(crate) fn setup_pod_network(
    pod_id: &str,
    ip: Ipv4Addr,
    subnet: &Ipv4Net,
    netns_path: &str,
    cluster_cidr: Option<&str>,
    service_cidr: Option<&str>,
) -> Result<()> {
    validate_netns_path(netns_path)?;
    if let Some(cidr) = cluster_cidr {
        validate_cidr(cidr)?;
    }
    if let Some(cidr) = service_cidr {
        validate_cidr(cidr)?;
    }
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

    // Apply egress rules (restricts cluster-internal traffic)
    if let Some(cidr) = cluster_cidr {
        setup_pod_egress_rules(netns_path, cidr, service_cidr)
            .with_context(|| format!("failed to set up egress rules for pod {pod_id}"))?;
    }

    info!(pod_id = %pod_id, "pod network setup complete");
    Ok(())
}

/// Tear down networking for a pod.
///
/// Deletes the host-side veth interface (the pod-side is cleaned up when
/// the network namespace is destroyed by the CRI).
pub(crate) fn teardown_pod_network(pod_id: &str) -> Result<()> {
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

/// Egress chain name for per-pod egress rules.
const EGRESS_CHAIN: &str = "SKYR-EGRESS";

/// Set up deny-all ingress firewall inside a pod's network namespace.
///
/// Rules:
/// - INPUT default policy: DROP
/// - Allow ESTABLISHED,RELATED (responses to outgoing traffic)
/// - Allow loopback
/// - OUTPUT default policy: ACCEPT (internet access)
/// - FORWARD default policy: DROP
/// - SKYR-EGRESS chain: jumped to from OUTPUT before cluster/service DROP rules
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

    // Create SKYR-EGRESS chain for attachment-managed egress rules
    nsenter_run(netns_path, "iptables", &["-N", EGRESS_CHAIN])?;

    Ok(())
}

/// Configure base egress rules for a pod.
///
/// By default pods can reach the internet but not other cluster-internal
/// destinations. Attachments dynamically add ACCEPT rules to the SKYR-EGRESS
/// chain for specific destinations.
///
/// Rules (appended to the OUTPUT chain, which defaults to ACCEPT):
/// 1. Allow loopback output
/// 2. Allow established/related outbound connections
/// 3. Jump to SKYR-EGRESS chain (for attachment-managed rules)
/// 4. Drop all traffic to the cluster CIDR (blocks cluster-internal)
/// 5. Drop all traffic to the service CIDR (blocks Host VIPs)
/// 6. Everything else (internet) falls through to the ACCEPT policy
fn setup_pod_egress_rules(
    netns_path: &str,
    cluster_cidr: &str,
    service_cidr: Option<&str>,
) -> Result<()> {
    validate_cidr(cluster_cidr)?;
    if let Some(cidr) = service_cidr {
        validate_cidr(cidr)?;
    }
    debug!(
        netns = %netns_path,
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

    // Jump to SKYR-EGRESS chain (before DROP rules)
    nsenter_run(
        netns_path,
        "iptables",
        &["-A", "OUTPUT", "-j", EGRESS_CHAIN],
    )?;

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
        "pod egress rules configured"
    );

    Ok(())
}

/// Open an egress port on a pod's firewall (for Attachment resources).
///
/// Appends an iptables ACCEPT rule to the SKYR-EGRESS chain in the pod's
/// network namespace, allowing outbound traffic to the specified destination.
pub(crate) fn open_egress_port(
    netns_path: &str,
    dest_address: &str,
    port: i32,
    protocol: &str,
) -> Result<()> {
    validate_netns_path(netns_path)?;
    validate_ipv4(dest_address)?;
    let port = validate_port(port)?;
    let protocol = validate_protocol(protocol)?;
    let port_str = port.to_string();
    info!(
        netns = %netns_path,
        dest_address = %dest_address,
        port = %port,
        protocol = %protocol,
        "opening egress port"
    );
    nsenter_run(
        netns_path,
        "iptables",
        &[
            "-A",
            EGRESS_CHAIN,
            "-d",
            dest_address,
            "-p",
            protocol,
            "--dport",
            &port_str,
            "-j",
            "ACCEPT",
        ],
    )
    .with_context(|| format!("failed to open egress port {dest_address}:{port}/{protocol}"))
}

/// Close an egress port on a pod's firewall (for Attachment resources).
///
/// Deletes the matching iptables ACCEPT rule from the SKYR-EGRESS chain
/// in the pod's network namespace.
pub(crate) fn close_egress_port(
    netns_path: &str,
    dest_address: &str,
    port: i32,
    protocol: &str,
) -> Result<()> {
    validate_netns_path(netns_path)?;
    validate_ipv4(dest_address)?;
    let port = validate_port(port)?;
    let protocol = validate_protocol(protocol)?;
    let port_str = port.to_string();
    info!(
        netns = %netns_path,
        dest_address = %dest_address,
        port = %port,
        protocol = %protocol,
        "closing egress port"
    );
    nsenter_run(
        netns_path,
        "iptables",
        &[
            "-D",
            EGRESS_CHAIN,
            "-d",
            dest_address,
            "-p",
            protocol,
            "--dport",
            &port_str,
            "-j",
            "ACCEPT",
        ],
    )
    .with_context(|| format!("failed to close egress port {dest_address}:{port}/{protocol}"))
}

// ============================================================================
// VXLAN overlay
// ============================================================================

/// Set up a VXLAN interface and attach it to the pod bridge.
///
/// This enables cross-node pod communication. The VXLAN uses the bridge's
/// L2 learning for MAC-to-VTEP resolution after BUM traffic is flooded
/// to all peers via FDB entries.
pub(crate) fn setup_vxlan(local_ip: &str) -> Result<()> {
    validate_ipv4(local_ip)?;
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
pub(crate) fn teardown_vxlan() -> Result<()> {
    info!(vxlan = VXLAN_NAME, "tearing down VXLAN overlay");
    let _ = run_cmd("ip", &["link", "del", VXLAN_NAME]);
    Ok(())
}

/// Add a VXLAN overlay peer.
///
/// Adds an FDB entry that directs BUM (broadcast, unknown-unicast, multicast)
/// traffic to the peer's underlay IP. This enables ARP resolution across nodes.
pub(crate) fn add_overlay_peer(peer_ip: &str) -> Result<()> {
    validate_ipv4(peer_ip)?;
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
pub(crate) fn remove_overlay_peer(peer_ip: &str) -> Result<()> {
    validate_ipv4(peer_ip)?;
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
pub(crate) fn open_port(netns_path: &str, port: i32, protocol: &str) -> Result<()> {
    validate_netns_path(netns_path)?;
    let port = validate_port(port)?;
    let protocol = validate_protocol(protocol)?;
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
pub(crate) fn close_port(netns_path: &str, port: i32, protocol: &str) -> Result<()> {
    validate_netns_path(netns_path)?;
    let port = validate_port(port)?;
    let protocol = validate_protocol(protocol)?;
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
pub(crate) fn setup_services_chain() -> Result<()> {
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

/// Tear down the services DNAT chain and all per-service chains.
pub(crate) fn teardown_services_chain() -> Result<()> {
    info!("tearing down SKYR-SERVICES iptables chain and per-service chains");
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

    // Clean up any per-service chains (SKYR_SVC_*) that were created by
    // add_service_route. Without this, chains leak across restarts.
    if let Ok(output) = std::process::Command::new("iptables")
        .args(["-t", "nat", "-L", "-n"])
        .output()
    {
        let listing = String::from_utf8_lossy(&output.stdout);
        let svc_chains: Vec<String> = listing
            .lines()
            .filter_map(|line| {
                let name = line.strip_prefix("Chain ")?.split_whitespace().next()?;
                if name.starts_with("SKYR_SVC_") {
                    Some(name.to_string())
                } else {
                    None
                }
            })
            .collect();
        for svc_chain in &svc_chains {
            let _ = run_cmd("iptables", &["-t", "nat", "-F", svc_chain]);
            let _ = run_cmd("iptables", &["-t", "nat", "-X", svc_chain]);
        }
    }

    Ok(())
}

/// Generate a per-service iptables chain name from VIP, port, and protocol.
///
/// The chain name encodes the service identity so that Host.Port chaining can
/// reference backend chains by jumping to them. Chain names use underscores
/// instead of dots/colons to satisfy iptables naming constraints.
///
/// Example: VIP `10.43.0.1`, port 80, protocol `tcp` → `SKYR_SVC_10_43_0_1_80_tcp`
fn service_chain_name(vip: &str, port: u16, protocol: &str) -> String {
    let vip_clean = vip.replace('.', "_");
    format!("SKYR_SVC_{vip_clean}_{port}_{protocol}")
}

/// Add a DNAT service route: VIP:port → load-balanced backends.
///
/// Creates a per-service iptables chain for the given VIP:port and populates
/// it with backend rules. A dispatch rule in `SKYR-SERVICES` jumps to the
/// per-service chain when traffic matches the VIP and port.
///
/// Backends whose address is within the service CIDR (i.e., another Host VIP)
/// are implemented as jumps to the corresponding per-service chain rather than
/// direct DNAT rules. This enables **Host.Port chaining**: a Host.Port can
/// list other Host.Ports as backends, and the iptables chains are traversed
/// in sequence until a terminal DNAT to a pod backend is reached.
///
/// For multiple backends, uses the iptables `statistic` module with
/// `--probability` for round-robin load balancing.
pub(crate) fn add_service_route(
    vip: &str,
    port: i32,
    protocol: &str,
    backends: &[scop::ServiceBackend],
    service_cidr: &str,
) -> Result<()> {
    validate_ipv4(vip)?;
    let port = validate_port(port)?;
    let protocol = validate_protocol(protocol)?;
    // Validate each backend address
    for backend in backends {
        validate_ipv4(&backend.address)?;
        validate_port(backend.port)?;
        validate_protocol(&backend.protocol)?;
    }

    if backends.is_empty() {
        warn!(vip = %vip, port = %port, "no backends for service route, skipping");
        return Ok(());
    }

    let port_str = port.to_string();
    let num_backends = backends.len();
    let chain = service_chain_name(vip, port, protocol);

    // Parse service CIDR once for VIP detection across all backends.
    let svc_net: Option<Ipv4Net> = service_cidr.parse().ok();

    info!(
        vip = %vip,
        port = %port,
        protocol = %protocol,
        num_backends = %num_backends,
        chain = %chain,
        "adding service route"
    );

    // Create the per-service chain (ignore error if it already exists).
    let _ = run_cmd("iptables", &["-t", "nat", "-N", &chain]);
    // Flush any existing rules (idempotent recreation).
    let _ = run_cmd("iptables", &["-t", "nat", "-F", &chain]);

    // Populate the per-service chain with backend rules.
    for (i, backend) in backends.iter().enumerate() {
        let is_vip = svc_net
            .as_ref()
            .and_then(|net| {
                backend
                    .address
                    .parse::<Ipv4Addr>()
                    .ok()
                    .map(|ip| net.contains(&ip))
            })
            .unwrap_or(false);

        // Build iptables args incrementally to avoid 4-way duplication.
        let mut args: Vec<&str> = vec!["-t", "nat", "-A", &chain];

        // For load balancing: use statistic --probability for all but the last backend.
        let probability;
        if i < num_backends - 1 {
            let remaining = num_backends - i;
            probability = format!("{:.10}", 1.0 / remaining as f64);
            args.extend_from_slice(&[
                "-m",
                "statistic",
                "--mode",
                "random",
                "--probability",
                &probability,
            ]);
        }

        // Jump target: chain jump for VIP backends, DNAT for pod backends.
        let backend_chain;
        let dest;
        if is_vip {
            backend_chain =
                service_chain_name(&backend.address, backend.port as u16, &backend.protocol);
            args.extend_from_slice(&["-j", &backend_chain]);
        } else {
            dest = format!("{}:{}", backend.address, backend.port);
            args.extend_from_slice(&["-j", "DNAT", "--to-destination", &dest]);
        }

        run_cmd("iptables", &args)
            .with_context(|| format!("failed to add backend rule in chain {chain}"))?;
    }

    // Ensure we don't accumulate duplicate dispatch rules on repeated calls.
    // Check-then-add: if the rule already exists (-C), skip the append.
    let dispatch_args = [
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
        &chain,
    ];
    let mut check_args = dispatch_args.to_vec();
    check_args[2] = "-C"; // -C (check) instead of -A (append)
    if run_cmd("iptables", &check_args).is_err() {
        run_cmd("iptables", &dispatch_args)
            .with_context(|| format!("failed to add dispatch rule for {vip}:{port}"))?;
    }

    info!(vip = %vip, port = %port, "service route added");
    Ok(())
}

/// Remove a DNAT service route for a VIP:port.
///
/// Removes the dispatch rule from SKYR-SERVICES and flushes/deletes the
/// per-service chain.
pub(crate) fn remove_service_route(vip: &str, port: i32, protocol: &str) -> Result<()> {
    validate_ipv4(vip)?;
    let port = validate_port(port)?;
    let protocol = validate_protocol(protocol)?;
    let port_str = port.to_string();
    let chain = service_chain_name(vip, port, protocol);

    info!(
        vip = %vip,
        port = %port,
        protocol = %protocol,
        chain = %chain,
        "removing service route"
    );

    // Remove dispatch rules in SKYR-SERVICES that jump to our per-service chain.
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
                && line.contains(&format!("-j {}", chain))
        });

        if let Some(rule) = rule_to_delete {
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

    // Flush and delete the per-service chain.
    let _ = run_cmd("iptables", &["-t", "nat", "-F", &chain]);
    let _ = run_cmd("iptables", &["-t", "nat", "-X", &chain]);

    info!(vip = %vip, port = %port, "service route removed");
    Ok(())
}

// ============================================================================
// DNS
// ============================================================================

/// Read the host's DNS configuration for forwarding to pods.
///
/// Returns a list of nameserver IPs found in /etc/resolv.conf.
pub(crate) fn host_nameservers() -> Vec<String> {
    let content = match std::fs::read_to_string("/etc/resolv.conf") {
        Ok(c) => c,
        Err(_) => return vec!["8.8.8.8".to_string()],
    };

    let servers: Vec<String> = content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with("nameserver") {
                let addr = line.split_whitespace().nth(1)?;
                // Validate that the nameserver is a well-formed IP address
                if addr.parse::<Ipv4Addr>().is_ok() || addr.parse::<std::net::Ipv6Addr>().is_ok() {
                    Some(addr.to_string())
                } else {
                    warn!(addr = %addr, "skipping invalid nameserver from resolv.conf");
                    None
                }
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
// VIP management (Internet Address exposure)
// ============================================================================

/// Detect the primary network interface by parsing the default route.
///
/// Runs `ip route show default` and extracts the `dev <iface>` field.
pub(crate) fn detect_primary_interface() -> Result<String> {
    let output = Command::new("ip")
        .args(["route", "show", "default"])
        .output()
        .context("failed to run 'ip route show default'")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("'ip route show default' failed: {}", stderr.trim());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Typical output: "default via 192.168.2.1 dev eth0 proto static"
    let iface = stdout
        .split_whitespace()
        .skip_while(|&token| token != "dev")
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("no 'dev' field in default route: {}", stdout.trim()))?
        .to_string();

    info!(interface = %iface, "detected primary network interface");
    Ok(iface)
}

/// Add a virtual IP address to the node's primary interface and send gratuitous ARP.
///
/// This only handles L2 reachability (ARP). Service routing for the VIP is handled
/// separately via `add_vip_dispatch`, which installs SKYR-SERVICES dispatch rules
/// so that traffic to the VIP is routed through the same per-service chains as the
/// Host VIP — achieving a single DNAT to backend pods.
pub(crate) fn add_vip_address(address: &str) -> Result<()> {
    let addr = validate_ipv4(address)?;
    let iface = detect_primary_interface()?;

    let addr_cidr = format!("{addr}/32");

    info!(vip = %addr, interface = %iface, "adding VIP address");

    // Enable arp_notify so the kernel sends a gratuitous ARP when the address is added
    let arp_notify_path = format!("/proc/sys/net/ipv4/conf/{iface}/arp_notify");
    std::fs::write(&arp_notify_path, "1")
        .with_context(|| format!("failed to enable arp_notify on {iface}"))?;

    // Add the VIP to the interface (triggers gratuitous ARP via arp_notify)
    run_cmd("ip", &["addr", "add", &addr_cidr, "dev", &iface])?;

    Ok(())
}

/// Remove a virtual IP address from the node's primary interface.
pub(crate) fn remove_vip_address(address: &str) -> Result<()> {
    let addr = validate_ipv4(address)?;
    let iface = detect_primary_interface()?;

    let addr_cidr = format!("{addr}/32");

    info!(vip = %addr, interface = %iface, "removing VIP address");

    run_cmd("ip", &["addr", "del", &addr_cidr, "dev", &iface])?;

    Ok(())
}

/// Add a SKYR-SERVICES dispatch rule for a VIP alias.
///
/// This adds a rule that dispatches traffic destined for `alias_vip:port` to the
/// same per-service chain used by `target_vip:port`. This avoids double-DNAT:
/// traffic arriving at the LAN VIP goes directly through SKYR-SERVICES to the
/// backend chain, resulting in a single DNAT to the actual backend pod.
pub(crate) fn add_vip_dispatch(
    alias_vip: &str,
    target_vip: &str,
    port: i32,
    protocol: &str,
) -> Result<()> {
    let port = validate_port(port)?;
    let protocol = validate_protocol(protocol)?;
    let port_str = port.to_string();
    let chain = service_chain_name(target_vip, port, protocol);

    info!(
        alias_vip = %alias_vip,
        target_vip = %target_vip,
        port = %port,
        protocol = %protocol,
        chain = %chain,
        "adding VIP dispatch alias"
    );

    // Check if the rule already exists before adding.
    let check_args = [
        "-t",
        "nat",
        "-C",
        SERVICES_CHAIN,
        "-d",
        alias_vip,
        "-p",
        protocol,
        "--dport",
        &port_str,
        "-j",
        &chain,
    ];
    if run_cmd("iptables", &check_args).is_err() {
        let add_args = [
            "-t",
            "nat",
            "-A",
            SERVICES_CHAIN,
            "-d",
            alias_vip,
            "-p",
            protocol,
            "--dport",
            &port_str,
            "-j",
            &chain,
        ];
        run_cmd("iptables", &add_args)
            .with_context(|| format!("failed to add VIP dispatch for {alias_vip}:{port}"))?;
    }

    Ok(())
}

/// Remove a SKYR-SERVICES dispatch rule for a VIP alias.
pub(crate) fn remove_vip_dispatch(
    alias_vip: &str,
    target_vip: &str,
    port: i32,
    protocol: &str,
) -> Result<()> {
    let port = validate_port(port)?;
    let protocol = validate_protocol(protocol)?;
    let port_str = port.to_string();
    let chain = service_chain_name(target_vip, port, protocol);

    info!(
        alias_vip = %alias_vip,
        target_vip = %target_vip,
        port = %port,
        protocol = %protocol,
        chain = %chain,
        "removing VIP dispatch alias"
    );

    let _ = run_cmd(
        "iptables",
        &[
            "-t",
            "nat",
            "-D",
            SERVICES_CHAIN,
            "-d",
            alias_vip,
            "-p",
            protocol,
            "--dport",
            &port_str,
            "-j",
            &chain,
        ],
    );

    Ok(())
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

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

// ============================================================================
// IPAM — per-node IP address management
// ============================================================================

/// Simple in-memory IPAM for a single node's subnet.
///
/// Allocates IPs sequentially from .2 upward (.1 is the gateway on the bridge).
/// Released IPs are recycled.
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
    pub fn gateway(&self) -> Ipv4Addr {
        self.gateway
    }

    /// The subnet managed by this IPAM.
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
    run_cmd("ip", &["link", "set", BRIDGE_NAME, "up"])
        .context("failed to bring bridge up")?;

    // Enable IP forwarding
    std::fs::write("/proc/sys/net/ipv4/ip_forward", "1")
        .context("failed to enable IP forwarding")?;

    // NAT: masquerade pod traffic going to the internet
    run_cmd(
        "iptables",
        &[
            "-t", "nat",
            "-A", "POSTROUTING",
            "-s", &subnet.to_string(),
            "!", "-o", BRIDGE_NAME,
            "-j", "MASQUERADE",
        ],
    )
    .context("failed to add MASQUERADE rule")?;

    // Allow forwarding from bridge
    run_cmd(
        "iptables",
        &[
            "-A", "FORWARD",
            "-i", BRIDGE_NAME,
            "-j", "ACCEPT",
        ],
    )
    .context("failed to add FORWARD accept rule for bridge")?;

    // Allow forwarding to bridge (for return traffic)
    run_cmd(
        "iptables",
        &[
            "-A", "FORWARD",
            "-o", BRIDGE_NAME,
            "-m", "conntrack",
            "--ctstate", "RELATED,ESTABLISHED",
            "-j", "ACCEPT",
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
            "-t", "nat",
            "-D", "POSTROUTING",
            "-s", &subnet.to_string(),
            "!", "-o", BRIDGE_NAME,
            "-j", "MASQUERADE",
        ],
    );
    let _ = run_cmd(
        "iptables",
        &[
            "-D", "FORWARD",
            "-i", BRIDGE_NAME,
            "-j", "ACCEPT",
        ],
    );
    let _ = run_cmd(
        "iptables",
        &[
            "-D", "FORWARD",
            "-o", BRIDGE_NAME,
            "-m", "conntrack",
            "--ctstate", "RELATED,ESTABLISHED",
            "-j", "ACCEPT",
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
/// installs deny-all ingress firewall rules.
pub fn setup_pod_network(
    pod_id: &str,
    ip: Ipv4Addr,
    subnet: &Ipv4Net,
    netns_path: &str,
) -> Result<()> {
    let gateway = Ipv4Addr::from(u32::from(subnet.network()) + 1);
    let ip_cidr = format!("{}/{}", ip, subnet.prefix_len());
    let host_veth = veth_host_name(pod_id);
    let pod_veth = "eth0";

    info!(
        pod_id = %pod_id,
        ip = %ip_cidr,
        host_veth = %host_veth,
        netns = %netns_path,
        "setting up pod network"
    );

    // Create veth pair
    run_cmd(
        "ip",
        &[
            "link", "add", &host_veth, "type", "veth",
            "peer", "name", pod_veth,
        ],
    )
    .with_context(|| format!("failed to create veth pair for pod {pod_id}"))?;

    // Attach host end to bridge
    run_cmd("ip", &["link", "set", &host_veth, "master", BRIDGE_NAME])
        .with_context(|| format!("failed to attach {host_veth} to bridge"))?;

    // Bring host end up
    run_cmd("ip", &["link", "set", &host_veth, "up"])
        .with_context(|| format!("failed to bring {host_veth} up"))?;

    // Move pod end into the network namespace
    run_cmd(
        "ip",
        &["link", "set", pod_veth, "netns", netns_path],
    )
    .with_context(|| format!("failed to move {pod_veth} into netns {netns_path}"))?;

    // Configure pod-side networking inside the namespace
    nsenter_run(netns_path, "ip", &["addr", "add", &ip_cidr, "dev", pod_veth])
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
            "-A", "INPUT",
            "-m", "conntrack",
            "--ctstate", "ESTABLISHED,RELATED",
            "-j", "ACCEPT",
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
    let mut nsenter_args = vec!["--net", netns_path, "--", program];
    nsenter_args.extend_from_slice(args);
    run_cmd("nsenter", &nsenter_args)
}

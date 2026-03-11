//! Lightweight DNS server for resolving `*.internal` hostnames to Host VIPs.
//!
//! This is a minimal DNS responder that runs on each SCOC node. It handles
//! A record queries for `*.internal` names from its in-memory record table,
//! and forwards all other queries to upstream DNS servers.
//!
//! The DNS server binds to the node's bridge gateway IP so pods can reach it
//! via their default gateway.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::{Arc, RwLock};

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

/// Shared DNS record table.
pub type DnsRecords = Arc<RwLock<HashMap<String, Ipv4Addr>>>;

/// Create a new shared DNS record table.
pub fn new_records() -> DnsRecords {
    Arc::new(RwLock::new(HashMap::new()))
}

/// DNS header flags and constants.
const DNS_FLAG_QR: u16 = 0x8000; // Response flag
const DNS_FLAG_AA: u16 = 0x0400; // Authoritative answer
const DNS_FLAG_RD: u16 = 0x0100; // Recursion desired
const DNS_FLAG_RA: u16 = 0x0080; // Recursion available
const DNS_RCODE_NXDOMAIN: u16 = 0x0003; // Name does not exist
const DNS_RCODE_SERVFAIL: u16 = 0x0002; // Server failure
const DNS_TYPE_A: u16 = 1;
const DNS_CLASS_IN: u16 = 1;

/// Run the DNS server on the given address (blocking, intended for tokio::task::spawn_blocking).
///
/// The server listens for UDP DNS queries, resolves `*.internal` names from
/// the record table, and forwards all other queries to upstream DNS.
pub fn run_dns_server(
    bind_addr: SocketAddr,
    records: DnsRecords,
    upstream_dns: Vec<String>,
) -> Result<()> {
    let socket = UdpSocket::bind(bind_addr).context("failed to bind DNS server")?;
    // Set a receive timeout so the thread can be interrupted
    socket.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;

    info!(addr = %bind_addr, "DNS server listening");

    let mut buf = [0u8; 512];
    loop {
        let (len, src) = match socket.recv_from(&mut buf) {
            Ok(result) => result,
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
            Err(e) => {
                warn!(error = %e, "DNS recv error");
                continue;
            }
        };

        let query = &buf[..len];
        if len < 12 {
            continue; // Too short to be a valid DNS message
        }

        match handle_query(query, &records, &upstream_dns) {
            Ok(response) => {
                if let Err(e) = socket.send_to(&response, src) {
                    warn!(error = %e, "DNS send error");
                }
            }
            Err(e) => {
                debug!(error = %e, "DNS query handling error");
                // Send SERVFAIL response
                if let Some(response) = build_error_response(query, DNS_RCODE_SERVFAIL) {
                    let _ = socket.send_to(&response, src);
                }
            }
        }
    }
}

/// Handle a DNS query. Returns the response bytes.
fn handle_query(query: &[u8], records: &DnsRecords, upstream_dns: &[String]) -> Result<Vec<u8>> {
    // Parse the question section to extract the queried name and type
    let (name, qtype, qclass, question_end) = parse_question(query)?;

    debug!(name = %name, qtype = qtype, "DNS query");

    // Only handle A record queries for *.internal names
    if qtype == DNS_TYPE_A && qclass == DNS_CLASS_IN && name.ends_with(".internal") {
        let records = records
            .read()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {e}"))?;

        if let Some(&ip) = records.get(&name) {
            debug!(name = %name, ip = %ip, "DNS: resolved from records");
            return build_a_response(query, question_end, &name, ip);
        }

        // Name is *.internal but not in our records → NXDOMAIN
        debug!(name = %name, "DNS: NXDOMAIN for unknown .internal name");
        return build_error_response(query, DNS_RCODE_NXDOMAIN)
            .ok_or_else(|| anyhow::anyhow!("failed to build NXDOMAIN response"));
    }

    // Forward to upstream DNS
    forward_query(query, upstream_dns)
}

/// Parse the DNS question section to extract name, type, and class.
/// Returns (name, qtype, qclass, end_offset).
fn parse_question(packet: &[u8]) -> Result<(String, u16, u16, usize)> {
    if packet.len() < 12 {
        anyhow::bail!("packet too short for DNS header");
    }

    let mut offset = 12; // Skip header
    let mut name_parts = Vec::new();

    loop {
        if offset >= packet.len() {
            anyhow::bail!("truncated question name");
        }
        let label_len = packet[offset] as usize;
        offset += 1;
        if label_len == 0 {
            break;
        }
        if offset + label_len > packet.len() {
            anyhow::bail!("truncated question label");
        }
        let label = std::str::from_utf8(&packet[offset..offset + label_len])
            .context("invalid UTF-8 in DNS label")?;
        name_parts.push(label.to_lowercase());
        offset += label_len;
    }

    if offset + 4 > packet.len() {
        anyhow::bail!("truncated question type/class");
    }

    let qtype = u16::from_be_bytes([packet[offset], packet[offset + 1]]);
    let qclass = u16::from_be_bytes([packet[offset + 2], packet[offset + 3]]);
    offset += 4;

    let name = name_parts.join(".");
    Ok((name, qtype, qclass, offset))
}

/// Build an A record response for a resolved name.
fn build_a_response(
    query: &[u8],
    question_end: usize,
    _name: &str,
    ip: Ipv4Addr,
) -> Result<Vec<u8>> {
    let mut response = Vec::with_capacity(question_end + 16);

    // Copy the header
    response.extend_from_slice(&query[..2]); // Transaction ID

    // Flags: QR=1 (response), AA=1 (authoritative), RD copied from query
    let query_flags = u16::from_be_bytes([query[2], query[3]]);
    let response_flags = DNS_FLAG_QR | DNS_FLAG_AA | DNS_FLAG_RA | (query_flags & DNS_FLAG_RD);
    response.extend_from_slice(&response_flags.to_be_bytes());

    // QDCOUNT=1
    response.extend_from_slice(&1u16.to_be_bytes());
    // ANCOUNT=1
    response.extend_from_slice(&1u16.to_be_bytes());
    // NSCOUNT=0, ARCOUNT=0
    response.extend_from_slice(&[0, 0, 0, 0]);

    // Copy the question section
    response.extend_from_slice(&query[12..question_end]);

    // Answer section: use name pointer to question (offset 12 in the packet)
    response.extend_from_slice(&[0xC0, 0x0C]); // Pointer to name at offset 12
    response.extend_from_slice(&DNS_TYPE_A.to_be_bytes()); // Type A
    response.extend_from_slice(&DNS_CLASS_IN.to_be_bytes()); // Class IN
    response.extend_from_slice(&60u32.to_be_bytes()); // TTL: 60 seconds
    response.extend_from_slice(&4u16.to_be_bytes()); // RDLENGTH: 4 bytes
    response.extend_from_slice(&ip.octets()); // RDATA: IPv4 address

    Ok(response)
}

/// Build an error response (NXDOMAIN, SERVFAIL, etc.).
fn build_error_response(query: &[u8], rcode: u16) -> Option<Vec<u8>> {
    if query.len() < 12 {
        return None;
    }

    let mut response = Vec::with_capacity(query.len());

    // Copy Transaction ID
    response.extend_from_slice(&query[..2]);

    // Flags: QR=1, rcode
    let query_flags = u16::from_be_bytes([query[2], query[3]]);
    let response_flags = DNS_FLAG_QR | DNS_FLAG_RA | (query_flags & DNS_FLAG_RD) | rcode;
    response.extend_from_slice(&response_flags.to_be_bytes());

    // QDCOUNT from original, ANCOUNT=0, NSCOUNT=0, ARCOUNT=0
    response.extend_from_slice(&query[4..6]); // QDCOUNT
    response.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // AN=0, NS=0, AR=0

    // Copy the question section verbatim
    response.extend_from_slice(&query[12..]);

    Some(response)
}

/// Forward a DNS query to upstream servers and return the response.
fn forward_query(query: &[u8], upstream_dns: &[String]) -> Result<Vec<u8>> {
    for upstream in upstream_dns {
        let upstream_addr: SocketAddr = format!("{upstream}:53")
            .parse()
            .with_context(|| format!("invalid upstream DNS address: {upstream}"))?;

        let sock = UdpSocket::bind("0.0.0.0:0").context("failed to bind forwarding socket")?;
        sock.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;

        if sock.send_to(query, upstream_addr).is_err() {
            continue;
        }

        let mut buf = [0u8; 512];
        match sock.recv_from(&mut buf) {
            Ok((len, _)) => return Ok(buf[..len].to_vec()),
            Err(_) => continue, // Try next upstream
        }
    }

    anyhow::bail!("all upstream DNS servers failed")
}

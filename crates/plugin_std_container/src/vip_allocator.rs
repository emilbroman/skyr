//! VIP (Virtual IP) allocator for Host resources.
//!
//! Allocates individual IP addresses from the service CIDR range for use as
//! Host VIPs. Each Host gets a unique VIP that serves as its cluster-internal
//! address for DNS resolution and DNAT routing.

use std::collections::HashMap;
use std::net::Ipv4Addr;

use ipnet::Ipv4Net;

/// Allocates single IP addresses from a service CIDR range.
///
/// VIPs start at .1 (there is no gateway for the service range — it's virtual).
/// The .0 (network) and broadcast addresses are reserved.
pub(crate) struct VipAllocator {
    /// The service CIDR range.
    service_cidr: Ipv4Net,
    /// Map from host name to allocated VIP.
    allocated: HashMap<String, Ipv4Addr>,
    /// Released VIPs available for reuse.
    free_pool: Vec<Ipv4Addr>,
    /// Next offset to allocate (when free_pool is empty).
    next_offset: u32,
}

impl VipAllocator {
    /// Create a new VIP allocator for the given service CIDR.
    pub(crate) fn new(service_cidr: Ipv4Net) -> Self {
        tracing::info!(
            service_cidr = %service_cidr,
            "VIP allocator initialized"
        );
        Self {
            service_cidr,
            allocated: HashMap::new(),
            free_pool: Vec::new(),
            next_offset: 1, // .0 is network, VIPs start at .1
        }
    }

    /// Allocate a VIP for a host. Returns the existing VIP if already allocated.
    pub(crate) fn allocate(&mut self, host_name: &str) -> Result<Ipv4Addr, String> {
        // Return existing allocation (idempotent)
        if let Some(&existing) = self.allocated.get(host_name) {
            return Ok(existing);
        }

        let ip = if let Some(ip) = self.free_pool.pop() {
            ip
        } else {
            let network_u32 = u32::from(self.service_cidr.network());
            let candidate = network_u32 + self.next_offset;
            let broadcast = u32::from(self.service_cidr.broadcast());
            if candidate >= broadcast {
                return Err(format!(
                    "VIP allocator exhausted: no more addresses in {}",
                    self.service_cidr
                ));
            }
            self.next_offset += 1;
            Ipv4Addr::from(candidate)
        };

        tracing::info!(
            host = %host_name,
            vip = %ip,
            "allocated VIP"
        );
        self.allocated.insert(host_name.to_string(), ip);
        Ok(ip)
    }

    /// Release a host's VIP back to the pool.
    pub(crate) fn release(&mut self, host_name: &str) {
        if let Some(ip) = self.allocated.remove(host_name) {
            tracing::info!(
                host = %host_name,
                vip = %ip,
                "released VIP"
            );
            self.free_pool.push(ip);
        }
    }

    #[cfg(test)]
    fn get(&self, host_name: &str) -> Option<Ipv4Addr> {
        self.allocated.get(host_name).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_sequential_vips() {
        let cidr: Ipv4Net = "10.43.0.0/16".parse().unwrap();
        let mut alloc = VipAllocator::new(cidr);

        let v1 = alloc.allocate("host-a").unwrap();
        let v2 = alloc.allocate("host-b").unwrap();
        let v3 = alloc.allocate("host-c").unwrap();

        assert_eq!(v1, Ipv4Addr::new(10, 43, 0, 1));
        assert_eq!(v2, Ipv4Addr::new(10, 43, 0, 2));
        assert_eq!(v3, Ipv4Addr::new(10, 43, 0, 3));
    }

    #[test]
    fn returns_existing_allocation() {
        let cidr: Ipv4Net = "10.43.0.0/16".parse().unwrap();
        let mut alloc = VipAllocator::new(cidr);

        let v1 = alloc.allocate("host-a").unwrap();
        let v1_again = alloc.allocate("host-a").unwrap();
        assert_eq!(v1, v1_again);
    }

    #[test]
    fn reuses_released_vips() {
        let cidr: Ipv4Net = "10.43.0.0/30".parse().unwrap();
        let mut alloc = VipAllocator::new(cidr);

        let v1 = alloc.allocate("host-a").unwrap();
        let v2 = alloc.allocate("host-b").unwrap();
        assert_eq!(v1, Ipv4Addr::new(10, 43, 0, 1));
        assert_eq!(v2, Ipv4Addr::new(10, 43, 0, 2));

        // Space exhausted
        assert!(alloc.allocate("host-c").is_err());

        // Release host-a
        alloc.release("host-a");

        // host-c gets the recycled VIP
        let v3 = alloc.allocate("host-c").unwrap();
        assert_eq!(v3, Ipv4Addr::new(10, 43, 0, 1));
    }

    #[test]
    fn get_returns_allocated_vip() {
        let cidr: Ipv4Net = "10.43.0.0/16".parse().unwrap();
        let mut alloc = VipAllocator::new(cidr);

        assert_eq!(alloc.get("host-a"), None);
        let v1 = alloc.allocate("host-a").unwrap();
        assert_eq!(alloc.get("host-a"), Some(v1));
    }
}

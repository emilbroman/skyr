//! Subnet allocator for per-node pod CIDRs.
//!
//! Divides a cluster-wide CIDR (e.g., `10.42.0.0/16`) into fixed-size node
//! subnets (e.g., `/24`) and assigns them to nodes during registration.

use std::collections::HashMap;
use std::net::Ipv4Addr;

use ipnet::Ipv4Net;

/// Allocates per-node subnets from a cluster CIDR.
///
/// Given a cluster CIDR like `10.42.0.0/16` and node prefix length `24`,
/// this allocator hands out subnets `10.42.0.0/24`, `10.42.1.0/24`, etc.
/// to each registering node.
pub struct SubnetAllocator {
    /// The cluster-wide CIDR (e.g., `10.42.0.0/16`).
    cluster_cidr: Ipv4Net,
    /// Prefix length for each node subnet (e.g., 24).
    node_prefix_len: u8,
    /// Total number of available node subnets.
    max_subnets: u32,
    /// Currently allocated subnets, keyed by node name.
    allocated: HashMap<String, Ipv4Net>,
    /// Next subnet index to try when allocating.
    next_index: u32,
}

impl SubnetAllocator {
    /// Create a new subnet allocator.
    ///
    /// - `cluster_cidr`: The overall cluster network (e.g., `10.42.0.0/16`)
    /// - `node_prefix_len`: The prefix length for per-node subnets (e.g., `24`)
    ///
    /// # Panics
    ///
    /// Panics if `node_prefix_len` is not larger than the cluster CIDR prefix
    /// length, or if `node_prefix_len > 30`.
    pub fn new(cluster_cidr: Ipv4Net, node_prefix_len: u8) -> Self {
        let cluster_prefix = cluster_cidr.prefix_len();
        assert!(
            node_prefix_len > cluster_prefix,
            "node prefix length ({node_prefix_len}) must be greater than cluster prefix ({cluster_prefix})"
        );
        assert!(
            node_prefix_len <= 30,
            "node prefix length must be <= 30 to leave room for host addresses"
        );

        let subnet_bits = node_prefix_len - cluster_prefix;
        let max_subnets = 1u32 << subnet_bits;

        tracing::info!(
            cluster_cidr = %cluster_cidr,
            node_prefix_len = node_prefix_len,
            max_subnets = max_subnets,
            "subnet allocator initialized"
        );

        Self {
            cluster_cidr,
            node_prefix_len,
            max_subnets,
            allocated: HashMap::new(),
            next_index: 0,
        }
    }

    /// Allocate a subnet for a node. Returns the allocated subnet.
    ///
    /// If the node already has an allocation, returns the existing one.
    pub fn allocate(&mut self, node_name: &str) -> Result<Ipv4Net, String> {
        // Return existing allocation if present
        if let Some(&existing) = self.allocated.get(node_name) {
            return Ok(existing);
        }

        // Find the next free subnet
        let allocated_subnets: std::collections::HashSet<u32> = self
            .allocated
            .values()
            .map(|net| self.subnet_index(net))
            .collect();

        let starting_index = self.next_index;
        loop {
            if !allocated_subnets.contains(&self.next_index) {
                let subnet = self.subnet_at_index(self.next_index);
                self.allocated.insert(node_name.to_string(), subnet);
                tracing::info!(
                    node = %node_name,
                    subnet = %subnet,
                    "allocated node subnet"
                );
                self.next_index = (self.next_index + 1) % self.max_subnets;
                return Ok(subnet);
            }
            self.next_index = (self.next_index + 1) % self.max_subnets;
            if self.next_index == starting_index {
                return Err(format!(
                    "no free subnets available in {} (all {} subnets allocated)",
                    self.cluster_cidr, self.max_subnets
                ));
            }
        }
    }

    /// Release a node's subnet allocation.
    pub fn release(&mut self, node_name: &str) {
        if let Some(subnet) = self.allocated.remove(node_name) {
            tracing::info!(
                node = %node_name,
                subnet = %subnet,
                "released node subnet"
            );
        }
    }

    /// Compute the subnet at a given index within the cluster CIDR.
    fn subnet_at_index(&self, index: u32) -> Ipv4Net {
        let base = u32::from(self.cluster_cidr.network());
        let host_bits = 32 - self.node_prefix_len;
        let subnet_addr = Ipv4Addr::from(base + (index << host_bits));
        Ipv4Net::new(subnet_addr, self.node_prefix_len).unwrap()
    }

    /// Compute the index of a subnet within the cluster CIDR.
    fn subnet_index(&self, subnet: &Ipv4Net) -> u32 {
        let base = u32::from(self.cluster_cidr.network());
        let addr = u32::from(subnet.network());
        let host_bits = 32 - self.node_prefix_len;
        (addr - base) >> host_bits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_sequential_subnets() {
        let cluster: Ipv4Net = "10.42.0.0/16".parse().unwrap();
        let mut alloc = SubnetAllocator::new(cluster, 24);

        let s1 = alloc.allocate("node-1").unwrap();
        let s2 = alloc.allocate("node-2").unwrap();
        let s3 = alloc.allocate("node-3").unwrap();

        assert_eq!(s1, "10.42.0.0/24".parse::<Ipv4Net>().unwrap());
        assert_eq!(s2, "10.42.1.0/24".parse::<Ipv4Net>().unwrap());
        assert_eq!(s3, "10.42.2.0/24".parse::<Ipv4Net>().unwrap());
    }

    #[test]
    fn returns_existing_allocation() {
        let cluster: Ipv4Net = "10.42.0.0/16".parse().unwrap();
        let mut alloc = SubnetAllocator::new(cluster, 24);

        let s1 = alloc.allocate("node-1").unwrap();
        let s1_again = alloc.allocate("node-1").unwrap();
        assert_eq!(s1, s1_again);
    }

    #[test]
    fn reuses_released_subnets() {
        let cluster: Ipv4Net = "10.0.0.0/30".parse().unwrap();
        // /30 -> /31 = 2 subnets (but /31 has only 2 addresses, that's fine for the test)
        let mut alloc = SubnetAllocator::new(cluster, 31);

        let s1 = alloc.allocate("node-1").unwrap();
        let _s2 = alloc.allocate("node-2").unwrap();

        // All subnets used
        assert!(alloc.allocate("node-3").is_err());

        // Release one
        alloc.release("node-1");

        // Now node-3 gets node-1's old subnet
        let s3 = alloc.allocate("node-3").unwrap();
        assert_eq!(s1, s3);
    }
}

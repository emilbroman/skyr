//! Tree-based subnet allocator for per-node pod CIDRs.
//!
//! Supports variable-size allocations from a cluster CIDR. Each node can
//! request a different prefix length (e.g., a beefy server gets /24 while
//! a Raspberry Pi gets /28). The allocator uses a binary trie to track
//! free/allocated space and find the smallest free block that fits.

use std::collections::HashMap;
use std::net::Ipv4Addr;

use ipnet::Ipv4Net;

/// A node in the subnet allocation tree.
///
/// The tree is a binary trie over the IP address space within the cluster CIDR.
/// Each level splits a prefix into two halves (left = 0 bit, right = 1 bit).
#[derive(Debug)]
enum Node {
    /// This entire block is free.
    Free,
    /// This entire block is allocated to a node.
    Allocated(String),
    /// This block is partially allocated; children represent the two halves.
    Split(Box<Node>, Box<Node>),
}

impl Node {
    /// Try to allocate a subnet of `target_prefix` length from a block at `current_prefix`.
    /// `base_addr` is the network address (as u32) of the current block.
    /// Returns the allocated subnet on success.
    fn allocate(
        &mut self,
        base_addr: u32,
        current_prefix: u8,
        target_prefix: u8,
        node_name: &str,
    ) -> Option<Ipv4Net> {
        match self {
            Node::Allocated(_) => None,

            Node::Free => {
                if current_prefix == target_prefix {
                    // Exact match — allocate this block
                    *self = Node::Allocated(node_name.to_string());
                    Some(Ipv4Net::new(Ipv4Addr::from(base_addr), current_prefix).unwrap())
                } else if current_prefix < target_prefix {
                    // Need to split this free block and allocate from the left half
                    let child_prefix = current_prefix + 1;
                    let mut left = Box::new(Node::Free);
                    let right = Box::new(Node::Free);

                    let result = left.allocate(base_addr, child_prefix, target_prefix, node_name);
                    *self = Node::Split(left, right);
                    result
                } else {
                    // current_prefix > target_prefix — block is too small
                    None
                }
            }

            Node::Split(left, right) => {
                // Try left first (lower addresses), then right
                if let Some(subnet) =
                    left.allocate(base_addr, current_prefix + 1, target_prefix, node_name)
                {
                    return Some(subnet);
                }
                let right_base = base_addr | (1 << (31 - current_prefix));
                right.allocate(right_base, current_prefix + 1, target_prefix, node_name)
            }
        }
    }

    /// Release a subnet belonging to `node_name`. Returns true if found and released.
    fn release(&mut self, node_name: &str) -> bool {
        match self {
            Node::Free => false,
            Node::Allocated(name) => {
                if name == node_name {
                    *self = Node::Free;
                    true
                } else {
                    false
                }
            }
            Node::Split(left, right) => {
                let released = left.release(node_name) || right.release(node_name);
                if released {
                    // Coalesce: if both children are now free, merge back
                    if matches!(left.as_ref(), Node::Free) && matches!(right.as_ref(), Node::Free) {
                        *self = Node::Free;
                    }
                }
                released
            }
        }
    }
}

/// Allocates variable-size per-node subnets from a cluster CIDR.
///
/// Uses a binary trie to efficiently find free space and supports different
/// prefix lengths per node. For example, with cluster CIDR `10.42.0.0/16`:
/// - Node A requests /24 → gets `10.42.0.0/24`
/// - Node B requests /28 → gets `10.42.1.0/28`
/// - Node C requests /24 → gets `10.42.2.0/24` (skips past B's /28 region)
pub(crate) struct SubnetAllocator {
    /// The cluster-wide CIDR.
    cluster_cidr: Ipv4Net,
    /// Root of the allocation tree.
    root: Node,
    /// Map from node name to allocated subnet, for quick lookups and idempotency.
    allocated: HashMap<String, Ipv4Net>,
}

impl SubnetAllocator {
    /// Create a new subnet allocator for the given cluster CIDR.
    pub(crate) fn new(cluster_cidr: Ipv4Net) -> Self {
        tracing::info!(
            cluster_cidr = %cluster_cidr,
            "subnet allocator initialized"
        );

        Self {
            cluster_cidr,
            root: Node::Free,
            allocated: HashMap::new(),
        }
    }

    /// Allocate a subnet of the given prefix length for a node.
    ///
    /// If the node already has an allocation, returns the existing one
    /// (ignoring the requested prefix length).
    pub(crate) fn allocate(&mut self, node_name: &str, prefix_len: u8) -> Result<Ipv4Net, String> {
        // Validate prefix length
        let cluster_prefix = self.cluster_cidr.prefix_len();
        if prefix_len <= cluster_prefix {
            return Err(format!(
                "requested prefix length ({prefix_len}) must be greater than cluster prefix ({cluster_prefix})"
            ));
        }
        if prefix_len > 30 {
            return Err(format!(
                "requested prefix length ({prefix_len}) must be <= 30"
            ));
        }

        // Return existing allocation if present (idempotent)
        if let Some(&existing) = self.allocated.get(node_name) {
            return Ok(existing);
        }

        let base_addr = u32::from(self.cluster_cidr.network());
        match self
            .root
            .allocate(base_addr, cluster_prefix, prefix_len, node_name)
        {
            Some(subnet) => {
                tracing::info!(
                    node = %node_name,
                    subnet = %subnet,
                    "allocated node subnet"
                );
                self.allocated.insert(node_name.to_string(), subnet);
                Ok(subnet)
            }
            None => Err(format!(
                "no free /{prefix_len} subnet available in {}",
                self.cluster_cidr
            )),
        }
    }

    /// Release a node's subnet allocation.
    pub(crate) fn release(&mut self, node_name: &str) {
        if self.allocated.remove(node_name).is_some() {
            self.root.release(node_name);
            tracing::info!(
                node = %node_name,
                "released node subnet"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_sequential_same_size() {
        let cluster: Ipv4Net = "10.42.0.0/16".parse().unwrap();
        let mut alloc = SubnetAllocator::new(cluster);

        let s1 = alloc.allocate("node-1", 24).unwrap();
        let s2 = alloc.allocate("node-2", 24).unwrap();
        let s3 = alloc.allocate("node-3", 24).unwrap();

        assert_eq!(s1, "10.42.0.0/24".parse::<Ipv4Net>().unwrap());
        assert_eq!(s2, "10.42.1.0/24".parse::<Ipv4Net>().unwrap());
        assert_eq!(s3, "10.42.2.0/24".parse::<Ipv4Net>().unwrap());
    }

    #[test]
    fn allocates_different_sizes() {
        let cluster: Ipv4Net = "10.0.0.0/16".parse().unwrap();
        let mut alloc = SubnetAllocator::new(cluster);

        // Allocate a /24
        let big = alloc.allocate("big-node", 24).unwrap();
        assert_eq!(big, "10.0.0.0/24".parse::<Ipv4Net>().unwrap());

        // Allocate a /28 — should come from the next available space
        let small = alloc.allocate("small-node", 28).unwrap();
        assert_eq!(small, "10.0.1.0/28".parse::<Ipv4Net>().unwrap());

        // Another /24 — should skip past the /28's parent block
        let big2 = alloc.allocate("big-node-2", 24).unwrap();
        assert_eq!(big2, "10.0.2.0/24".parse::<Ipv4Net>().unwrap());
    }

    #[test]
    fn returns_existing_allocation() {
        let cluster: Ipv4Net = "10.42.0.0/16".parse().unwrap();
        let mut alloc = SubnetAllocator::new(cluster);

        let s1 = alloc.allocate("node-1", 24).unwrap();
        let s1_again = alloc.allocate("node-1", 28).unwrap(); // different size, still returns existing
        assert_eq!(s1, s1_again);
    }

    #[test]
    fn reuses_released_subnets() {
        let cluster: Ipv4Net = "10.0.0.0/24".parse().unwrap();
        let mut alloc = SubnetAllocator::new(cluster);

        let s1 = alloc.allocate("node-1", 25).unwrap();
        let s2 = alloc.allocate("node-2", 25).unwrap();
        assert_eq!(s1, "10.0.0.0/25".parse::<Ipv4Net>().unwrap());
        assert_eq!(s2, "10.0.0.128/25".parse::<Ipv4Net>().unwrap());

        // All space used for /25s
        assert!(alloc.allocate("node-3", 25).is_err());

        // Release node-1
        alloc.release("node-1");

        // node-3 gets node-1's old subnet
        let s3 = alloc.allocate("node-3", 25).unwrap();
        assert_eq!(s3, "10.0.0.0/25".parse::<Ipv4Net>().unwrap());
    }

    #[test]
    fn coalesces_on_release() {
        let cluster: Ipv4Net = "10.0.0.0/24".parse().unwrap();
        let mut alloc = SubnetAllocator::new(cluster);

        // Allocate two /25s that fill the /24
        let _s1 = alloc.allocate("node-1", 25).unwrap();
        let _s2 = alloc.allocate("node-2", 25).unwrap();

        // Release both
        alloc.release("node-1");
        alloc.release("node-2");

        // Now a full /25 should be available again at the start
        let s3 = alloc.allocate("node-3", 25).unwrap();
        assert_eq!(s3, "10.0.0.0/25".parse::<Ipv4Net>().unwrap());
    }

    #[test]
    fn rejects_invalid_prefix() {
        let cluster: Ipv4Net = "10.42.0.0/16".parse().unwrap();
        let mut alloc = SubnetAllocator::new(cluster);

        // prefix <= cluster prefix
        assert!(alloc.allocate("node-1", 16).is_err());
        assert!(alloc.allocate("node-1", 15).is_err());

        // prefix > 30
        assert!(alloc.allocate("node-1", 31).is_err());
    }

    #[test]
    fn small_allocation_from_split_block() {
        let cluster: Ipv4Net = "10.0.0.0/24".parse().unwrap();
        let mut alloc = SubnetAllocator::new(cluster);

        // Allocate a /28 (16 addresses)
        let s1 = alloc.allocate("node-1", 28).unwrap();
        assert_eq!(s1, "10.0.0.0/28".parse::<Ipv4Net>().unwrap());

        // Allocate another /28 — should fit in the same /27 half
        let s2 = alloc.allocate("node-2", 28).unwrap();
        assert_eq!(s2, "10.0.0.16/28".parse::<Ipv4Net>().unwrap());

        // Allocate a /26 — the left /25 has a free /26 in its upper half (10.0.0.64/26)
        let s3 = alloc.allocate("node-3", 26).unwrap();
        assert_eq!(s3, "10.0.0.64/26".parse::<Ipv4Net>().unwrap());

        // Allocate another /26 — now goes to the right /25
        let s4 = alloc.allocate("node-4", 26).unwrap();
        assert_eq!(s4, "10.0.0.128/26".parse::<Ipv4Net>().unwrap());
    }
}

//! Node Database
//!
//! Redis-backed registry for tracking worker nodes in the Skyr cluster.
//! Nodes register themselves with their addresses so the plugin can connect to them.

use redis::{AsyncCommands, Client as RedisClient};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

/// Microseconds since the Unix epoch, used for overlay gossip `last_seen_micros`
/// timestamps. Treated as monotonic across the single orchestrator process.
pub(crate) fn now_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

#[derive(Error, Debug)]
pub(crate) enum ConnectError {
    #[error("failed to create redis client: {0}")]
    RedisClient(#[from] redis::RedisError),

    #[error("failed to connect to redis server: {0}")]
    RedisConnection(#[source] redis::RedisError),
}

#[derive(Default)]
pub(crate) struct ClientBuilder {
    known_nodes: Vec<String>,
}

impl ClientBuilder {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn known_node(mut self, hostname: impl AsRef<str>) -> Self {
        self.known_nodes.push(hostname.as_ref().to_owned());
        self
    }

    pub(crate) async fn build(&self) -> Result<Client, ConnectError> {
        let node = self
            .known_nodes
            .first()
            .map(String::as_str)
            .unwrap_or("127.0.0.1");
        let url = format!("redis://{node}/");

        let redis_client = RedisClient::open(url)?;
        let conn = redis_client
            .get_multiplexed_async_connection()
            .await
            .map_err(ConnectError::RedisConnection)?;

        Ok(Client { conn })
    }
}

#[derive(Error, Debug)]
pub(crate) enum NodeError {
    #[error("failed to execute query: {0}")]
    Redis(#[from] redis::RedisError),

    #[error("node not found: {0}")]
    NotFound(String),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// Resource capacity of a worker node.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct NodeCapacity {
    /// CPU capacity in millicores (e.g., 4000 = 4 cores).
    pub cpu_millis: i64,
    /// Memory capacity in bytes.
    pub memory_bytes: i64,
    /// Maximum number of pods.
    pub max_pods: i32,
}

/// Current resource usage of a worker node.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct NodeUsage {
    /// CPU usage in millicores.
    pub cpu_millis: i64,
    /// Memory usage in bytes.
    pub memory_bytes: i64,
    /// Current number of running pods.
    pub running_pods: i32,
}

/// Information about a registered worker node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Node {
    /// Unique name of the node.
    pub name: String,
    /// Conduit address of the node (e.g., "http://node-1:50054").
    pub address: String,
    /// Resource capacity.
    pub capacity: NodeCapacity,
    /// Current resource usage.
    pub usage: NodeUsage,
    /// Labels attached to the node for scheduling purposes.
    pub labels: HashMap<String, String>,
    /// Last heartbeat timestamp (Unix epoch seconds).
    pub last_heartbeat: u64,
    /// Pod CIDR assigned to this node (e.g., "10.42.1.0/24").
    #[serde(default)]
    pub pod_cidr: String,
    /// Host IP for VXLAN overlay underlay traffic.
    #[serde(default)]
    pub overlay_endpoint: String,
    /// Microseconds since Unix epoch at which the orchestrator last minted this
    /// node's overlay membership record. Used as `last_seen_micros` in the
    /// overlay gossip protocol so that peer entries carry a monotonic version
    /// stamp that supersedes any older tombstone with the same `name`.
    #[serde(default)]
    pub overlay_version_micros: u64,
}

/// Tombstone record for an evicted or unregistered node. Persisted in Redis
/// so seed lists sent to newly-joining nodes can include recent removals, and
/// so admin tooling can inspect the decommissioning state of the cluster.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Tombstone {
    /// Canonical hostname of the removed node.
    pub name: String,
    /// The overlay endpoint the node had at the time of eviction, so peers
    /// have enough information to drop the matching FDB entry if they still
    /// carry a stale live entry under this `name`.
    pub overlay_endpoint: String,
    /// Microseconds since Unix epoch at which the orchestrator minted this
    /// tombstone. Must be strictly greater than the live entry it replaces.
    pub last_seen_micros: u64,
    /// Microseconds since Unix epoch after which this tombstone may be GC'd
    /// by the orchestrator; in-cluster GC is performed independently by each
    /// SCOC based on its configured `--tombstone-ttl`.
    pub expires_at_micros: u64,
}

#[derive(Clone)]
pub(crate) struct Client {
    conn: redis::aio::MultiplexedConnection,
}

const PREFIX_NODE: &str = "n:";
const PREFIX_VIP: &str = "vip:";
const PREFIX_TOMBSTONE: &str = "ts:";
const SET_NODES: &str = "nodes";
const SET_VIPS: &str = "vips";
const SET_TOMBSTONES: &str = "tombstones";

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl Client {
    /// Register a node with its address and capacity.
    ///
    /// `overlay_version_micros` must be strictly greater than the
    /// `last_seen_micros` of any tombstone carrying the same `name` so the
    /// fresh registration supersedes it during gossip merges. The caller
    /// (typically the orchestrator's `register_node` handler) is responsible
    /// for picking this value — usually `now_micros()` — since only the
    /// caller can coordinate with tombstone state.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn register(
        &mut self,
        name: impl Into<String>,
        address: impl Into<String>,
        capacity: NodeCapacity,
        labels: HashMap<String, String>,
        pod_cidr: impl Into<String>,
        overlay_endpoint: impl Into<String>,
        overlay_version_micros: u64,
    ) -> Result<Node, NodeError> {
        let name = name.into();
        let node = Node {
            name: name.clone(),
            address: address.into(),
            capacity,
            usage: NodeUsage::default(),
            labels,
            last_heartbeat: now_secs(),
            pod_cidr: pod_cidr.into(),
            overlay_endpoint: overlay_endpoint.into(),
            overlay_version_micros,
        };

        let data = serde_json::to_string(&node)?;

        // Store the node data
        let _: () = self.conn.set(format!("{PREFIX_NODE}{name}"), &data).await?;

        // Add to the set of nodes
        let _: () = self.conn.sadd(SET_NODES, &name).await?;

        Ok(node)
    }

    /// Update a node's heartbeat timestamp and usage.
    pub(crate) async fn heartbeat(
        &mut self,
        name: impl AsRef<str>,
        usage: Option<NodeUsage>,
    ) -> Result<Node, NodeError> {
        let name = name.as_ref();
        let mut node = self.get(name).await?;

        node.last_heartbeat = now_secs();
        if let Some(usage) = usage {
            node.usage = usage;
        }

        let data = serde_json::to_string(&node)?;
        let _: () = self.conn.set(format!("{PREFIX_NODE}{name}"), &data).await?;

        Ok(node)
    }

    /// Remove a node from the registry.
    pub(crate) async fn unregister(&mut self, name: impl AsRef<str>) -> Result<(), NodeError> {
        let name = name.as_ref();
        let key = format!("{PREFIX_NODE}{name}");

        // Remove the node data
        let _: () = self.conn.del(&key).await?;

        // Remove from the set of nodes
        let _: () = self.conn.srem(SET_NODES, name).await?;

        Ok(())
    }

    /// Get a node by name.
    pub(crate) async fn get(&mut self, name: impl AsRef<str>) -> Result<Node, NodeError> {
        let name = name.as_ref();
        let key = format!("{PREFIX_NODE}{name}");

        let data: Option<String> = self.conn.get(&key).await?;
        let Some(data) = data else {
            return Err(NodeError::NotFound(name.to_owned()));
        };

        let node: Node = serde_json::from_str(&data)?;
        Ok(node)
    }

    /// List all registered nodes.
    pub(crate) async fn list(&mut self) -> Result<Vec<Node>, NodeError> {
        let names: Vec<String> = self.conn.smembers(SET_NODES).await?;
        let mut nodes = Vec::with_capacity(names.len());

        for name in names {
            match self.get(&name).await {
                Ok(node) => nodes.push(node),
                Err(NodeError::NotFound(_)) => {
                    // Node was removed between listing and fetching, skip it
                    continue;
                }
                Err(e) => return Err(e),
            }
        }

        Ok(nodes)
    }

    /// List nodes whose last heartbeat is older than the given threshold (in seconds).
    pub(crate) async fn stale_nodes(&mut self, max_age_secs: u64) -> Result<Vec<Node>, NodeError> {
        let cutoff = now_secs().saturating_sub(max_age_secs);
        let all = self.list().await?;
        Ok(all
            .into_iter()
            .filter(|n| n.last_heartbeat < cutoff)
            .collect())
    }

    /// Store a VIP allocation in Redis for persistence across restarts.
    pub(crate) async fn store_vip(&mut self, host_name: &str, vip: &str) -> Result<(), NodeError> {
        let _: () = self
            .conn
            .set(format!("{PREFIX_VIP}{host_name}"), vip)
            .await?;
        let _: () = self.conn.sadd(SET_VIPS, host_name).await?;
        Ok(())
    }

    /// Remove a VIP allocation from Redis.
    pub(crate) async fn remove_vip(&mut self, host_name: &str) -> Result<(), NodeError> {
        let _: () = self.conn.del(format!("{PREFIX_VIP}{host_name}")).await?;
        let _: () = self.conn.srem(SET_VIPS, host_name).await?;
        Ok(())
    }

    /// List all stored VIP allocations (host_name → VIP address string).
    pub(crate) async fn list_vips(&mut self) -> Result<Vec<(String, String)>, NodeError> {
        let names: Vec<String> = self.conn.smembers(SET_VIPS).await?;
        let mut vips = Vec::with_capacity(names.len());

        for name in names {
            let key = format!("{PREFIX_VIP}{name}");
            let vip: Option<String> = self.conn.get(&key).await?;
            if let Some(vip) = vip {
                vips.push((name, vip));
            }
        }

        Ok(vips)
    }

    /// Write (or overwrite) a tombstone for a removed node.
    pub(crate) async fn put_tombstone(&mut self, tombstone: &Tombstone) -> Result<(), NodeError> {
        let data = serde_json::to_string(tombstone)?;
        let _: () = self
            .conn
            .set(format!("{PREFIX_TOMBSTONE}{}", tombstone.name), &data)
            .await?;
        let _: () = self.conn.sadd(SET_TOMBSTONES, &tombstone.name).await?;
        Ok(())
    }

    /// Fetch a tombstone by node name, if any.
    #[allow(dead_code)]
    pub(crate) async fn get_tombstone(
        &mut self,
        name: impl AsRef<str>,
    ) -> Result<Option<Tombstone>, NodeError> {
        let name = name.as_ref();
        let data: Option<String> = self.conn.get(format!("{PREFIX_TOMBSTONE}{name}")).await?;
        Ok(match data {
            Some(s) => Some(serde_json::from_str(&s)?),
            None => None,
        })
    }

    /// Delete a tombstone by node name (no-op if absent).
    pub(crate) async fn delete_tombstone(
        &mut self,
        name: impl AsRef<str>,
    ) -> Result<(), NodeError> {
        let name = name.as_ref();
        let _: () = self.conn.del(format!("{PREFIX_TOMBSTONE}{name}")).await?;
        let _: () = self.conn.srem(SET_TOMBSTONES, name).await?;
        Ok(())
    }

    /// List all non-expired tombstones, dropping (and deleting) any whose
    /// `expires_at_micros` is in the past.
    pub(crate) async fn list_tombstones(&mut self) -> Result<Vec<Tombstone>, NodeError> {
        let names: Vec<String> = self.conn.smembers(SET_TOMBSTONES).await?;
        let now = now_micros();
        let mut tombstones = Vec::with_capacity(names.len());

        for name in names {
            let data: Option<String> = self.conn.get(format!("{PREFIX_TOMBSTONE}{name}")).await?;
            let Some(data) = data else {
                let _: () = self.conn.srem(SET_TOMBSTONES, &name).await?;
                continue;
            };
            let tombstone: Tombstone = serde_json::from_str(&data)?;
            if tombstone.expires_at_micros <= now {
                let _: () = self.conn.del(format!("{PREFIX_TOMBSTONE}{name}")).await?;
                let _: () = self.conn.srem(SET_TOMBSTONES, &name).await?;
                continue;
            }
            tombstones.push(tombstone);
        }

        Ok(tombstones)
    }
}

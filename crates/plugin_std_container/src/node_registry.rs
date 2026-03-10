//! Node Database
//!
//! Redis-backed registry for tracking worker nodes in the Skyr cluster.
//! Nodes register themselves with their addresses so the plugin can connect to them.

use redis::{AsyncCommands, Client as RedisClient};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConnectError {
    #[error("failed to create redis client: {0}")]
    RedisClient(#[from] redis::RedisError),

    #[error("failed to connect to redis server: {0}")]
    RedisConnection(#[source] redis::RedisError),
}

#[derive(Default)]
pub struct ClientBuilder {
    known_nodes: Vec<String>,
}

impl ClientBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn known_node(mut self, hostname: impl AsRef<str>) -> Self {
        self.known_nodes.push(hostname.as_ref().to_owned());
        self
    }

    pub async fn build(&self) -> Result<Client, ConnectError> {
        let node = self
            .known_nodes
            .first()
            .cloned()
            .unwrap_or_else(|| "127.0.0.1".to_owned());
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
pub enum NodeError {
    #[error("failed to execute query: {0}")]
    Redis(#[from] redis::RedisError),

    #[error("node not found: {0}")]
    NotFound(String),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// Resource capacity of a worker node.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NodeCapacity {
    /// CPU capacity in millicores (e.g., 4000 = 4 cores).
    pub cpu_millis: i64,
    /// Memory capacity in bytes.
    pub memory_bytes: i64,
    /// Maximum number of pods.
    pub max_pods: i32,
}

/// Current resource usage of a worker node.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NodeUsage {
    /// CPU usage in millicores.
    pub cpu_millis: i64,
    /// Memory usage in bytes.
    pub memory_bytes: i64,
    /// Current number of running pods.
    pub running_pods: i32,
}

/// Information about a registered worker node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
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
}

#[derive(Clone)]
pub struct Client {
    conn: redis::aio::MultiplexedConnection,
}

const PREFIX_NODE: &str = "n:";
const SET_NODES: &str = "nodes";

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl Client {
    /// Register a node with its address and capacity.
    pub async fn register(
        &mut self,
        name: impl Into<String>,
        address: impl Into<String>,
        capacity: NodeCapacity,
        labels: HashMap<String, String>,
        pod_cidr: impl Into<String>,
        overlay_endpoint: impl Into<String>,
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
        };

        let data = serde_json::to_string(&node)?;

        // Store the node data
        let _: () = self.conn.set(format!("{PREFIX_NODE}{name}"), &data).await?;

        // Add to the set of nodes
        let _: () = self.conn.sadd(SET_NODES, &name).await?;

        Ok(node)
    }

    /// Update a node's heartbeat timestamp and usage.
    pub async fn heartbeat(
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
    pub async fn unregister(&mut self, name: impl AsRef<str>) -> Result<(), NodeError> {
        let name = name.as_ref();
        let key = format!("{PREFIX_NODE}{name}");

        // Remove the node data
        let _: () = self.conn.del(&key).await?;

        // Remove from the set of nodes
        let _: () = self.conn.srem(SET_NODES, name).await?;

        Ok(())
    }

    /// Get a node by name.
    pub async fn get(&mut self, name: impl AsRef<str>) -> Result<Node, NodeError> {
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
    pub async fn list(&mut self) -> Result<Vec<Node>, NodeError> {
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
}

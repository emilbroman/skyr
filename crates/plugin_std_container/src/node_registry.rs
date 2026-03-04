//! Node Database
//!
//! Redis-backed registry for tracking worker nodes in the Skyr cluster.

use redis::{AsyncCommands, Client as RedisClient};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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

/// Information about a registered worker node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    /// Unique name of the node.
    pub name: String,
    /// Labels attached to the node for scheduling purposes.
    pub labels: HashMap<String, String>,
    /// Whether the node is currently connected.
    pub connected: bool,
}

#[derive(Clone)]
pub struct Client {
    conn: redis::aio::MultiplexedConnection,
}

const PREFIX_NODE: &str = "n:";
const SET_NODES: &str = "nodes";

impl Client {
    /// Register a node as connected. If the node already exists, updates
    /// its connection state and labels.
    pub async fn register(
        &mut self,
        name: impl Into<String>,
        labels: HashMap<String, String>,
    ) -> Result<Node, NodeError> {
        let name = name.into();
        let node = Node {
            name: name.clone(),
            labels,
            connected: true,
        };

        let data = serde_json::to_string(&node)?;

        // Store the node data
        let _: () = self
            .conn
            .set(format!("{PREFIX_NODE}{name}"), &data)
            .await?;

        // Add to the set of nodes
        let _: () = self.conn.sadd(SET_NODES, &name).await?;

        Ok(node)
    }

    /// Mark a node as disconnected. Does not remove the node from the registry.
    pub async fn disconnect(&mut self, name: impl AsRef<str>) -> Result<(), NodeError> {
        let name = name.as_ref();
        let key = format!("{PREFIX_NODE}{name}");

        // Get current node data
        let data: Option<String> = self.conn.get(&key).await?;
        let Some(data) = data else {
            return Err(NodeError::NotFound(name.to_owned()));
        };

        let mut node: Node = serde_json::from_str(&data)?;
        node.connected = false;

        let data = serde_json::to_string(&node)?;
        let _: () = self.conn.set(&key, &data).await?;

        Ok(())
    }
}

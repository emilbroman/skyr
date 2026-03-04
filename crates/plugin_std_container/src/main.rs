//! Container Plugin for Skyr
//!
//! This plugin manages container workloads across a cluster of worker nodes.
//! It implements SCOP to communicate with SCOC agents on worker nodes.
//!
//! Phase 3: SCOP server and node registration
//! Phase 4 (TODO): RTP server for resource management

mod node_registry;

use std::collections::HashMap;

use anyhow::Result;
use clap::Parser;
use tokio::sync::RwLock;
use tracing::{error, info};

#[derive(Parser)]
struct Args {
    /// Address to bind the SCOP server (e.g., "0.0.0.0:50053")
    #[arg(long)]
    bind: String,

    /// Node registry hostname (Redis)
    #[arg(long)]
    node_registry_hostname: String,

    /// BuildKit server address (Phase 6)
    #[arg(long)]
    buildkit_addr: String,

    /// Container registry URL (Phase 6)
    #[arg(long)]
    registry_url: String,
}

/// The container orchestrator manages connected nodes and their handles.
struct ContainerOrchestrator {
    /// Node registry client for persisting node state.
    node_registry: RwLock<node_registry::Client>,
    /// In-memory map of connected node handles, keyed by node name.
    handles: RwLock<HashMap<String, scop::NodeHandle>>,
}

impl ContainerOrchestrator {
    fn new(node_registry: node_registry::Client) -> Self {
        Self {
            node_registry: RwLock::new(node_registry),
            handles: RwLock::new(HashMap::new()),
        }
    }

    /// Get a handle to a connected node by name.
    #[allow(dead_code)]
    pub async fn get_node(&self, name: &str) -> Option<scop::NodeHandle> {
        let handles = self.handles.read().await;
        handles.get(name).cloned()
    }

    /// List all connected node names.
    #[allow(dead_code)]
    pub async fn list_connected_nodes(&self) -> Vec<String> {
        let handles = self.handles.read().await;
        handles.keys().cloned().collect()
    }
}

#[scop::tonic::async_trait]
impl scop::Orchestrator for ContainerOrchestrator {
    async fn on_node_registered(&self, handle: scop::NodeHandle) -> bool {
        let node_name = handle.node_name.clone();
        let labels = handle.labels.clone();

        info!(
            node_name = %node_name,
            labels = ?labels,
            "node registration request"
        );

        // Register in node registry
        {
            let mut registry = self.node_registry.write().await;
            if let Err(e) = registry.register(&node_name, labels).await {
                error!(
                    node_name = %node_name,
                    err = %e,
                    "failed to register node in registry"
                );
                return false;
            }
        }

        // Store handle in memory
        {
            let mut handles = self.handles.write().await;
            handles.insert(node_name.clone(), handle);
        }

        info!(node_name = %node_name, "node registered successfully");
        true
    }

    async fn on_node_disconnected(&self, node_name: &str) {
        info!(node_name = %node_name, "node disconnected");

        // Remove from in-memory handles
        {
            let mut handles = self.handles.write().await;
            handles.remove(node_name);
        }

        // Mark as disconnected in node registry
        {
            let mut registry = self.node_registry.write().await;
            if let Err(e) = registry.disconnect(node_name).await {
                error!(
                    node_name = %node_name,
                    err = %e,
                    "failed to mark node as disconnected in registry"
                );
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    info!("Container plugin starting");
    info!("  bind: {}", args.bind);
    info!("  node_registry_hostname: {}", args.node_registry_hostname);
    info!("  buildkit_addr: {}", args.buildkit_addr);
    info!("  registry_url: {}", args.registry_url);

    // Connect to the node registry
    let node_registry = node_registry::ClientBuilder::new()
        .known_node(&args.node_registry_hostname)
        .build()
        .await?;

    info!("Connected to node registry");

    // Create the orchestrator
    let orchestrator = ContainerOrchestrator::new(node_registry);

    // Start the SCOP server
    info!(bind = %args.bind, "Starting SCOP server");
    scop::serve(&args.bind, orchestrator).await?;

    Ok(())
}

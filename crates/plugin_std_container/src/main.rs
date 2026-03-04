//! Container Plugin for Skyr
//!
//! This plugin manages container workloads across a cluster of worker nodes.
//! It serves as the Orchestrator, accepting node registrations and connecting
//! to Conduit services to execute container operations.
//!
//! Phase 3: Orchestrator service and node registry
//! Phase 4 (TODO): RTP server for resource management

mod node_registry;

use anyhow::Result;
use clap::Parser;
use tokio::sync::RwLock;
use tracing::info;

#[derive(Parser)]
struct Args {
    /// Address to bind the Orchestrator server to.
    #[arg(long, default_value = "0.0.0.0:50053")]
    bind: String,

    /// Node registry hostname (Redis).
    #[arg(long)]
    node_registry_hostname: String,

    /// BuildKit server address (Phase 6).
    #[arg(long)]
    buildkit_addr: String,

    /// Container registry URL (Phase 6).
    #[arg(long)]
    registry_url: String,
}

/// The container plugin manages connections to worker nodes.
pub struct ContainerPlugin {
    /// Node registry client for storing and looking up node addresses.
    node_registry: RwLock<node_registry::Client>,
}

impl ContainerPlugin {
    fn new(node_registry: node_registry::Client) -> Self {
        Self {
            node_registry: RwLock::new(node_registry),
        }
    }

    /// Get a conduit client to a node by name.
    pub async fn get_conduit(
        &self,
        node_name: &str,
    ) -> Result<scop::ConduitClient<scop::tonic::transport::Channel>, PluginError> {
        // Look up the node address
        let node = {
            let mut registry = self.node_registry.write().await;
            registry
                .get(node_name)
                .await
                .map_err(|e| PluginError::NodeLookup(e.to_string()))?
        };

        // Connect to the node's conduit service
        info!(node_name = %node_name, address = %node.address, "connecting to conduit");
        let client = scop::ConduitClient::connect(node.address.clone())
            .await
            .map_err(|e| PluginError::Connect(e.to_string()))?;

        Ok(client)
    }

    /// List all registered nodes.
    pub async fn list_nodes(&self) -> Result<Vec<node_registry::Node>, PluginError> {
        let mut registry = self.node_registry.write().await;
        registry
            .list()
            .await
            .map_err(|e| PluginError::NodeLookup(e.to_string()))
    }
}

#[scop::tonic::async_trait]
impl scop::Orchestrator for ContainerPlugin {
    async fn register_node(
        &self,
        request: scop::RegisterNodeRequest,
    ) -> Result<scop::RegisterNodeResponse, scop::tonic::Status> {
        info!(
            node_name = %request.node_name,
            conduit_address = %request.conduit_address,
            "registering node"
        );

        let capacity = request.capacity.unwrap_or_default();
        let node_capacity = node_registry::NodeCapacity {
            cpu_millis: capacity.cpu_millis,
            memory_bytes: capacity.memory_bytes,
            max_pods: capacity.max_pods,
        };

        let mut registry = self.node_registry.write().await;
        match registry
            .register(
                &request.node_name,
                &request.conduit_address,
                node_capacity,
                request.labels,
            )
            .await
        {
            Ok(node) => {
                info!(node_name = %node.name, "node registered successfully");
                Ok(scop::RegisterNodeResponse {
                    success: true,
                    error: String::new(),
                })
            }
            Err(e) => {
                let error = e.to_string();
                tracing::error!(node_name = %request.node_name, error = %error, "failed to register node");
                Ok(scop::RegisterNodeResponse {
                    success: false,
                    error,
                })
            }
        }
    }

    async fn heartbeat(
        &self,
        request: scop::HeartbeatRequest,
    ) -> Result<scop::HeartbeatResponse, scop::tonic::Status> {
        let usage = request.usage.map(|u| node_registry::NodeUsage {
            cpu_millis: u.cpu_millis,
            memory_bytes: u.memory_bytes,
            running_pods: u.running_pods,
        });

        let mut registry = self.node_registry.write().await;
        match registry.heartbeat(&request.node_name, usage).await {
            Ok(_) => Ok(scop::HeartbeatResponse { acknowledged: true }),
            Err(e) => {
                tracing::warn!(
                    node_name = %request.node_name,
                    error = %e,
                    "heartbeat failed"
                );
                Ok(scop::HeartbeatResponse { acknowledged: false })
            }
        }
    }

    async fn unregister_node(
        &self,
        request: scop::UnregisterNodeRequest,
    ) -> Result<scop::UnregisterNodeResponse, scop::tonic::Status> {
        info!(node_name = %request.node_name, "unregistering node");

        let mut registry = self.node_registry.write().await;
        match registry.unregister(&request.node_name).await {
            Ok(()) => {
                info!(node_name = %request.node_name, "node unregistered successfully");
                Ok(scop::UnregisterNodeResponse { success: true })
            }
            Err(e) => {
                tracing::error!(
                    node_name = %request.node_name,
                    error = %e,
                    "failed to unregister node"
                );
                Ok(scop::UnregisterNodeResponse { success: false })
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("node lookup failed: {0}")]
    NodeLookup(String),

    #[error("failed to connect to node: {0}")]
    Connect(String),
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

    // Create the plugin
    let plugin = ContainerPlugin::new(node_registry);

    // Start the Orchestrator server
    let bind_target = format!("http://{}", args.bind);
    info!("Starting Orchestrator server on {}", args.bind);

    // Phase 4 TODO: Also start RTP server here

    scop::serve_orchestrator(&bind_target, plugin).await?;

    Ok(())
}

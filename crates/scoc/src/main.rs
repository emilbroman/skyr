use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tokio::sync::Mutex;

mod cri;

use cri::CriClient;

#[derive(Parser)]
#[command(name = "scoc")]
#[command(about = "Skyr Container Orchestrator Conduit")]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the SCOC conduit daemon.
    Daemon {
        /// Unique name for this node.
        #[arg(long)]
        node_name: String,
        /// Address to bind the Conduit server to.
        #[arg(long, default_value = "0.0.0.0:50054")]
        bind: String,
        /// External address for the plugin to connect to (e.g., "http://node-1:50054").
        #[arg(long)]
        conduit_address: String,
        /// Orchestrator address (container plugin).
        #[arg(long)]
        orchestrator_address: String,
        #[arg(long, default_value = "/run/containerd/containerd.sock")]
        containerd_socket: String,
        /// CPU capacity in millicores (e.g., 4000 = 4 cores).
        #[arg(long, default_value = "4000")]
        cpu_millis: i64,
        /// Memory capacity in bytes.
        #[arg(long, default_value = "8589934592")]
        memory_bytes: i64,
        /// Maximum number of pods.
        #[arg(long, default_value = "100")]
        max_pods: i32,
    },
    /// Check CRI connectivity and version.
    Version {
        #[arg(long, default_value = "/run/containerd/containerd.sock")]
        containerd_socket: String,
    },
    /// Pod operations for testing.
    Pod {
        #[command(subcommand)]
        action: PodAction,
    },
    /// Container operations for testing.
    Container {
        #[command(subcommand)]
        action: ContainerAction,
    },
}

#[derive(Subcommand)]
enum PodAction {
    /// Create and run a test pod.
    Run {
        /// Pod name.
        #[arg(long)]
        name: String,
        /// Pod namespace.
        #[arg(long, default_value = "default")]
        namespace: String,
        #[arg(long, default_value = "/run/containerd/containerd.sock")]
        containerd_socket: String,
    },
    /// Stop a pod.
    Stop {
        /// Pod ID.
        #[arg(long)]
        id: String,
        #[arg(long, default_value = "/run/containerd/containerd.sock")]
        containerd_socket: String,
    },
    /// Remove a pod.
    Remove {
        /// Pod ID.
        #[arg(long)]
        id: String,
        #[arg(long, default_value = "/run/containerd/containerd.sock")]
        containerd_socket: String,
    },
}

#[derive(Subcommand)]
enum ContainerAction {
    /// Create a container in a pod.
    Create {
        /// Pod ID.
        #[arg(long)]
        pod_id: String,
        /// Container name.
        #[arg(long)]
        name: String,
        /// Container image.
        #[arg(long)]
        image: String,
        #[arg(long, default_value = "/run/containerd/containerd.sock")]
        containerd_socket: String,
    },
    /// Start a container.
    Start {
        /// Container ID.
        #[arg(long)]
        id: String,
        #[arg(long, default_value = "/run/containerd/containerd.sock")]
        containerd_socket: String,
    },
    /// Stop a container.
    Stop {
        /// Container ID.
        #[arg(long)]
        id: String,
        /// Timeout in seconds.
        #[arg(long, default_value = "10")]
        timeout: i64,
        #[arg(long, default_value = "/run/containerd/containerd.sock")]
        containerd_socket: String,
    },
    /// Remove a container.
    Remove {
        /// Container ID.
        #[arg(long)]
        id: String,
        #[arg(long, default_value = "/run/containerd/containerd.sock")]
        containerd_socket: String,
    },
}

/// SCOP Conduit implementation backed by CRI.
struct CriConduit {
    cri: Arc<Mutex<CriClient>>,
}

impl CriConduit {
    fn new(cri: CriClient) -> Self {
        Self {
            cri: Arc::new(Mutex::new(cri)),
        }
    }

    /// Convert SCOP PodConfig to CRI PodSandboxConfig.
    fn to_cri_pod_config(config: &scop::PodConfig) -> k8s_cri::v1::PodSandboxConfig {
        let metadata = config.metadata.as_ref();
        cri::test_pod_config(
            metadata.map(|m| m.name.as_str()).unwrap_or("unknown"),
            metadata.map(|m| m.namespace.as_str()).unwrap_or("default"),
        )
    }

    /// Convert SCOP ContainerConfig to CRI ContainerConfig.
    fn to_cri_container_config(config: &scop::ContainerConfig) -> k8s_cri::v1::ContainerConfig {
        let name = config
            .metadata
            .as_ref()
            .map(|m| m.name.as_str())
            .unwrap_or("unknown");
        let image = &config.image;

        let mut cri_config = cri::test_container_config(name, image);

        // Set command if provided
        if !config.command.is_empty() {
            cri_config.command = config.command.clone();
        }

        // Set args if provided
        if !config.args.is_empty() {
            cri_config.args = config.args.clone();
        }

        // Set environment variables
        if !config.envs.is_empty() {
            cri_config.envs = config
                .envs
                .iter()
                .map(|kv| k8s_cri::v1::KeyValue {
                    key: kv.key.clone(),
                    value: kv.value.clone(),
                })
                .collect();
        }

        // Set labels
        if !config.labels.is_empty() {
            cri_config.labels = config.labels.clone();
        }

        // Set annotations
        if !config.annotations.is_empty() {
            cri_config.annotations = config.annotations.clone();
        }

        cri_config
    }
}

#[scop::tonic::async_trait]
impl scop::Conduit for CriConduit {
    async fn run_pod(
        &self,
        request: scop::RunPodRequest,
    ) -> Result<scop::RunPodResponse, scop::tonic::Status> {
        let config = request.config.unwrap_or_default();
        let cri_config = Self::to_cri_pod_config(&config);
        let mut cri = self.cri.lock().await;
        let pod_id = cri
            .run_pod_sandbox(cri_config)
            .await
            .map_err(|e| scop::tonic::Status::internal(e.to_string()))?;
        Ok(scop::RunPodResponse { pod_id })
    }

    async fn stop_pod(
        &self,
        request: scop::StopPodRequest,
    ) -> Result<scop::StopPodResponse, scop::tonic::Status> {
        let mut cri = self.cri.lock().await;
        cri.stop_pod_sandbox(&request.pod_id)
            .await
            .map_err(|e| scop::tonic::Status::internal(e.to_string()))?;
        Ok(scop::StopPodResponse {})
    }

    async fn remove_pod(
        &self,
        request: scop::RemovePodRequest,
    ) -> Result<scop::RemovePodResponse, scop::tonic::Status> {
        let mut cri = self.cri.lock().await;
        cri.remove_pod_sandbox(&request.pod_id)
            .await
            .map_err(|e| scop::tonic::Status::internal(e.to_string()))?;
        Ok(scop::RemovePodResponse {})
    }

    async fn create_container(
        &self,
        request: scop::CreateContainerRequest,
    ) -> Result<scop::CreateContainerResponse, scop::tonic::Status> {
        let config = request.config.unwrap_or_default();
        let pod_config = request.pod_config.unwrap_or_default();
        let cri_pod_config = Self::to_cri_pod_config(&pod_config);
        let cri_container_config = Self::to_cri_container_config(&config);
        let mut cri = self.cri.lock().await;

        // Pull the image first to ensure it's available in the CRI namespace
        cri.pull_image(&config.image, None)
            .await
            .map_err(|e| scop::tonic::Status::internal(format!("failed to pull image: {}", e)))?;

        let container_id = cri
            .create_container(&request.pod_id, &cri_pod_config, cri_container_config)
            .await
            .map_err(|e| scop::tonic::Status::internal(e.to_string()))?;
        Ok(scop::CreateContainerResponse { container_id })
    }

    async fn start_container(
        &self,
        request: scop::StartContainerRequest,
    ) -> Result<scop::StartContainerResponse, scop::tonic::Status> {
        let mut cri = self.cri.lock().await;
        cri.start_container(&request.container_id)
            .await
            .map_err(|e| scop::tonic::Status::internal(e.to_string()))?;
        Ok(scop::StartContainerResponse {})
    }

    async fn stop_container(
        &self,
        request: scop::StopContainerRequest,
    ) -> Result<scop::StopContainerResponse, scop::tonic::Status> {
        let mut cri = self.cri.lock().await;
        cri.stop_container(&request.container_id, request.timeout)
            .await
            .map_err(|e| scop::tonic::Status::internal(e.to_string()))?;
        Ok(scop::StopContainerResponse {})
    }

    async fn remove_container(
        &self,
        request: scop::RemoveContainerRequest,
    ) -> Result<scop::RemoveContainerResponse, scop::tonic::Status> {
        let mut cri = self.cri.lock().await;
        cri.remove_container(&request.container_id)
            .await
            .map_err(|e| scop::tonic::Status::internal(e.to_string()))?;
        Ok(scop::RemoveContainerResponse {})
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    match args.command {
        Command::Daemon {
            node_name,
            bind,
            conduit_address,
            orchestrator_address,
            containerd_socket,
            cpu_millis,
            memory_bytes,
            max_pods,
        } => {
            tracing::info!("SCOC conduit starting");
            tracing::info!("  node_name: {}", node_name);
            tracing::info!("  bind: {}", bind);
            tracing::info!("  conduit_address: {}", conduit_address);
            tracing::info!("  orchestrator_address: {}", orchestrator_address);
            tracing::info!("  containerd_socket: {}", containerd_socket);

            // Verify CRI connectivity at startup
            let cri = {
                let mut cri = CriClient::connect(&containerd_socket).await?;
                let version = cri.version().await?;
                tracing::info!("containerd version: {}", version);
                cri
            };

            // Create the conduit
            let conduit = CriConduit::new(cri);

            // Connect to orchestrator and register
            tracing::info!("Registering with orchestrator at {}", orchestrator_address);
            let mut orchestrator =
                scop::OrchestratorClient::connect(orchestrator_address.clone()).await?;

            let register_response = orchestrator
                .register_node(scop::RegisterNodeRequest {
                    node_name: node_name.clone(),
                    conduit_address: conduit_address.clone(),
                    capacity: Some(scop::NodeCapacity {
                        cpu_millis,
                        memory_bytes,
                        max_pods,
                    }),
                    labels: Default::default(),
                })
                .await?
                .into_inner();

            if !register_response.success {
                anyhow::bail!(
                    "Failed to register with orchestrator: {}",
                    register_response.error
                );
            }
            tracing::info!("Registered with orchestrator");

            // Spawn heartbeat task
            let node_name_heartbeat = node_name.clone();
            let orchestrator_address_heartbeat = orchestrator_address.clone();
            let heartbeat_handle = tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(30)).await;

                    match scop::OrchestratorClient::connect(orchestrator_address_heartbeat.clone())
                        .await
                    {
                        Ok(mut client) => {
                            if let Err(e) = client
                                .heartbeat(scop::HeartbeatRequest {
                                    node_name: node_name_heartbeat.clone(),
                                    usage: None,
                                })
                                .await
                            {
                                tracing::warn!("Heartbeat failed: {}", e);
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Failed to connect for heartbeat: {}", e);
                        }
                    }
                }
            });

            // Start Conduit server in a separate task
            let bind_target = format!("http://{}", bind);
            let server_handle = tokio::spawn(async move {
                scop::serve_conduit(&bind_target, conduit).await
            });

            // Wait for shutdown signal
            tokio::select! {
                result = server_handle => {
                    if let Err(e) = result {
                        tracing::error!("Conduit server error: {}", e);
                    }
                }
                _ = tokio::signal::ctrl_c() => {
                    tracing::info!("Received shutdown signal");
                }
            }

            // Cancel heartbeat task
            heartbeat_handle.abort();

            // Unregister from orchestrator
            tracing::info!("Unregistering from orchestrator");
            if let Ok(mut client) =
                scop::OrchestratorClient::connect(orchestrator_address).await
            {
                if let Err(e) = client
                    .unregister_node(scop::UnregisterNodeRequest {
                        node_name: node_name.clone(),
                    })
                    .await
                {
                    tracing::error!("Failed to unregister: {}", e);
                }
            }
        }

        Command::Version { containerd_socket } => {
            let mut cri = CriClient::connect(&containerd_socket).await?;
            let version = cri.version().await?;
            println!("Runtime version: {version}");
        }

        Command::Pod { action } => match action {
            PodAction::Run {
                name,
                namespace,
                containerd_socket,
            } => {
                let mut cri = CriClient::connect(&containerd_socket).await?;
                let config = cri::test_pod_config(&name, &namespace);
                let pod_id = cri.run_pod_sandbox(config).await?;
                println!("{pod_id}");
            }
            PodAction::Stop {
                id,
                containerd_socket,
            } => {
                let mut cri = CriClient::connect(&containerd_socket).await?;
                cri.stop_pod_sandbox(&id).await?;
                println!("Pod stopped");
            }
            PodAction::Remove {
                id,
                containerd_socket,
            } => {
                let mut cri = CriClient::connect(&containerd_socket).await?;
                cri.remove_pod_sandbox(&id).await?;
                println!("Pod removed");
            }
        },

        Command::Container { action } => match action {
            ContainerAction::Create {
                pod_id,
                name,
                image,
                containerd_socket,
            } => {
                let mut cri = CriClient::connect(&containerd_socket).await?;

                // Pull the image first to ensure it's in the CRI namespace
                cri.pull_image(&image, None).await?;

                // Create a minimal pod config for the container creation call
                let pod_config = cri::test_pod_config("pod", "default");
                let container_config = cri::test_container_config(&name, &image);
                let container_id = cri
                    .create_container(&pod_id, &pod_config, container_config)
                    .await?;
                println!("{container_id}");
            }
            ContainerAction::Start {
                id,
                containerd_socket,
            } => {
                let mut cri = CriClient::connect(&containerd_socket).await?;
                cri.start_container(&id).await?;
                println!("Container started");
            }
            ContainerAction::Stop {
                id,
                timeout,
                containerd_socket,
            } => {
                let mut cri = CriClient::connect(&containerd_socket).await?;
                cri.stop_container(&id, timeout).await?;
                println!("Container stopped");
            }
            ContainerAction::Remove {
                id,
                containerd_socket,
            } => {
                let mut cri = CriClient::connect(&containerd_socket).await?;
                cri.remove_container(&id).await?;
                println!("Container removed");
            }
        },
    }

    Ok(())
}

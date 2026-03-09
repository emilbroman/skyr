use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

mod cri;
mod log_stream;

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
        /// LDB broker address for container log streaming.
        #[arg(long, default_value = "127.0.0.1:9092")]
        ldb_brokers: String,
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
    /// Create a test pod.
    Create {
        /// Pod name.
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "/run/containerd/containerd.sock")]
        containerd_socket: String,
    },
    /// Stop and remove a pod.
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

/// Tracked pod info for log streaming namespace construction.
struct PodInfo {
    environment_qid: String,
    name: String,
}

/// Tracked container info for log streaming lifecycle.
struct ContainerInfo {
    pod_id: String,
    name: String,
}

/// SCOP Conduit implementation backed by CRI.
struct CriConduit {
    cri: Arc<Mutex<CriClient>>,
    ldb_publisher: Option<ldb::Publisher>,
    log_tasks: Arc<Mutex<HashMap<String, CancellationToken>>>,
    pods: Arc<Mutex<HashMap<String, PodInfo>>>,
    containers: Arc<Mutex<HashMap<String, ContainerInfo>>>,
}

impl CriConduit {
    fn new(cri: CriClient, ldb_publisher: Option<ldb::Publisher>) -> Self {
        Self {
            cri: Arc::new(Mutex::new(cri)),
            ldb_publisher,
            log_tasks: Arc::new(Mutex::new(HashMap::new())),
            pods: Arc::new(Mutex::new(HashMap::new())),
            containers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Convert SCOP PodConfig to CRI PodSandboxConfig.
    fn to_cri_pod_config(config: &scop::PodConfig) -> k8s_cri::v1::PodSandboxConfig {
        cri::pod_sandbox_config(&config.name)
    }

    /// Convert SCOP ContainerConfig to CRI ContainerConfig.
    fn to_cri_container_config(config: &scop::ContainerConfig) -> k8s_cri::v1::ContainerConfig {
        let mut cri_config = cri::container_config(&config.name, &config.image);

        if !config.command.is_empty() {
            cri_config.command = config.command.clone();
        }

        if !config.args.is_empty() {
            cri_config.args = config.args.clone();
        }

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

        cri_config
    }

    /// Start a log streaming task for a container.
    async fn start_log_streaming(&self, container_id: &str) {
        let publisher = match &self.ldb_publisher {
            Some(p) => p.clone(),
            None => return,
        };

        let (pod_name, environment_qid, container_name) = {
            let containers = self.containers.lock().await;
            let Some(container_info) = containers.get(container_id) else {
                tracing::warn!(
                    container_id = %container_id,
                    "no container info found for log streaming"
                );
                return;
            };
            let pods = self.pods.lock().await;
            let Some(pod_info) = pods.get(&container_info.pod_id) else {
                tracing::warn!(
                    container_id = %container_id,
                    pod_id = %container_info.pod_id,
                    "no pod info found for log streaming"
                );
                return;
            };
            (
                pod_info.name.clone(),
                pod_info.environment_qid.clone(),
                container_info.name.clone(),
            )
        };

        // LDB namespace: {environment_qid}::{pod_name}/{container_name}
        let ldb_namespace = format!("{environment_qid}::{pod_name}/{container_name}");

        let namespace_publisher = match publisher.namespace(ldb_namespace.clone()).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(
                    container_id = %container_id,
                    ldb_namespace = %ldb_namespace,
                    error = %e,
                    "failed to create LDB namespace publisher for container logs"
                );
                return;
            }
        };

        // Log path: /var/log/pods/skyr_{pod_name}/{container_name}/0.log
        let log_path = PathBuf::from(format!(
            "/var/log/pods/skyr_{pod_name}/{container_name}/0.log"
        ));

        let cancel = CancellationToken::new();
        {
            let mut tasks = self.log_tasks.lock().await;
            tasks.insert(container_id.to_string(), cancel.clone());
        }

        tracing::info!(
            container_id = %container_id,
            ldb_namespace = %ldb_namespace,
            log_path = %log_path.display(),
            "starting container log streaming"
        );

        tokio::spawn(async move {
            log_stream::stream_container_logs(log_path, namespace_publisher, cancel).await;
        });
    }

    /// Cancel a log streaming task for a container.
    async fn cancel_log_streaming(&self, container_id: &str) {
        let mut tasks = self.log_tasks.lock().await;
        if let Some(cancel) = tasks.remove(container_id) {
            tracing::info!(
                container_id = %container_id,
                "cancelling container log streaming"
            );
            cancel.cancel();
        }
    }
}

#[scop::tonic::async_trait]
impl scop::Conduit for CriConduit {
    async fn create_pod(
        &self,
        request: scop::CreatePodRequest,
    ) -> Result<scop::CreatePodResponse, scop::tonic::Status> {
        let config = request.config.unwrap_or_default();
        let cri_config = Self::to_cri_pod_config(&config);
        let mut cri = self.cri.lock().await;
        let pod_id = cri
            .run_pod_sandbox(cri_config)
            .await
            .map_err(|e| scop::tonic::Status::internal(e.to_string()))?;

        // Track pod info for log streaming
        {
            let mut pods = self.pods.lock().await;
            pods.insert(
                pod_id.clone(),
                PodInfo {
                    environment_qid: config.environment_qid,
                    name: config.name,
                },
            );
        }

        Ok(scop::CreatePodResponse { pod_id })
    }

    async fn remove_pod(
        &self,
        request: scop::RemovePodRequest,
    ) -> Result<scop::RemovePodResponse, scop::tonic::Status> {
        let mut cri = self.cri.lock().await;

        // Stop the pod sandbox first, then remove it
        cri.stop_pod_sandbox(&request.pod_id)
            .await
            .map_err(|e| scop::tonic::Status::internal(e.to_string()))?;
        cri.remove_pod_sandbox(&request.pod_id)
            .await
            .map_err(|e| scop::tonic::Status::internal(e.to_string()))?;

        // Clean up pod info
        {
            let mut pods = self.pods.lock().await;
            pods.remove(&request.pod_id);
        }

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

        // Track container info for log streaming
        {
            let mut containers = self.containers.lock().await;
            containers.insert(
                container_id.clone(),
                ContainerInfo {
                    pod_id: request.pod_id,
                    name: config.name,
                },
            );
        }

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
        drop(cri);

        // Start log streaming after container starts
        self.start_log_streaming(&request.container_id).await;

        Ok(scop::StartContainerResponse {})
    }

    async fn stop_container(
        &self,
        request: scop::StopContainerRequest,
    ) -> Result<scop::StopContainerResponse, scop::tonic::Status> {
        // Cancel log streaming before stopping
        self.cancel_log_streaming(&request.container_id).await;

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
        // Cancel log streaming if still running
        self.cancel_log_streaming(&request.container_id).await;

        let mut cri = self.cri.lock().await;
        cri.remove_container(&request.container_id)
            .await
            .map_err(|e| scop::tonic::Status::internal(e.to_string()))?;

        // Clean up container info
        {
            let mut containers = self.containers.lock().await;
            containers.remove(&request.container_id);
        }

        Ok(scop::RemoveContainerResponse {})
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
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
            ldb_brokers,
        } => {
            tracing::info!("SCOC conduit starting");
            tracing::info!("  node_name: {}", node_name);
            tracing::info!("  bind: {}", bind);
            tracing::info!("  conduit_address: {}", conduit_address);
            tracing::info!("  orchestrator_address: {}", orchestrator_address);
            tracing::info!("  containerd_socket: {}", containerd_socket);
            tracing::info!("  ldb_brokers: {}", ldb_brokers);

            // Verify CRI connectivity at startup
            let cri = {
                let mut cri = CriClient::connect(&containerd_socket).await?;
                let version = cri.version().await?;
                tracing::info!("containerd version: {}", version);
                cri
            };

            // Connect to LDB for container log streaming
            let ldb_publisher = match ldb::ClientBuilder::new()
                .brokers(ldb_brokers)
                .build_publisher()
                .await
            {
                Ok(publisher) => {
                    tracing::info!("Connected to LDB for container log streaming");
                    Some(publisher)
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to connect to LDB, container log streaming disabled: {}",
                        e
                    );
                    None
                }
            };

            // Create the conduit
            let conduit = CriConduit::new(cri, ldb_publisher);

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
            PodAction::Create {
                name,
                containerd_socket,
            } => {
                let mut cri = CriClient::connect(&containerd_socket).await?;
                let config = cri::pod_sandbox_config(&name);
                let pod_id = cri.run_pod_sandbox(config).await?;
                println!("{pod_id}");
            }
            PodAction::Remove {
                id,
                containerd_socket,
            } => {
                let mut cri = CriClient::connect(&containerd_socket).await?;
                cri.stop_pod_sandbox(&id).await?;
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
                let pod_config = cri::pod_sandbox_config("pod");
                let container_config = cri::container_config(&name, &image);
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

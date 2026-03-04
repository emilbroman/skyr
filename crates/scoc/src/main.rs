use std::collections::HashMap;

use anyhow::Result;
use clap::{Parser, Subcommand};

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
    /// Run the SCOC agent daemon.
    Daemon {
        #[arg(long)]
        node_name: String,
        #[arg(long)]
        plugin_addr: String,
        #[arg(long, default_value = "/run/containerd/containerd.sock")]
        containerd_socket: String,
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

/// SCOP Agent implementation backed by CRI.
struct CriAgent {
    cri: CriClient,
}

impl CriAgent {
    fn new(cri: CriClient) -> Self {
        Self { cri }
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
impl scop::Agent for CriAgent {
    async fn run_pod(
        &mut self,
        config: scop::PodConfig,
    ) -> anyhow::Result<scop::RunPodResponse> {
        let cri_config = Self::to_cri_pod_config(&config);
        let pod_id = self.cri.run_pod_sandbox(cri_config).await?;
        Ok(scop::RunPodResponse { pod_id })
    }

    async fn stop_pod(&mut self, pod_id: String) -> anyhow::Result<()> {
        self.cri.stop_pod_sandbox(&pod_id).await
    }

    async fn remove_pod(&mut self, pod_id: String) -> anyhow::Result<()> {
        self.cri.remove_pod_sandbox(&pod_id).await
    }

    async fn create_container(
        &mut self,
        pod_id: String,
        config: scop::ContainerConfig,
        pod_config: scop::PodConfig,
    ) -> anyhow::Result<scop::CreateContainerResponse> {
        let cri_pod_config = Self::to_cri_pod_config(&pod_config);
        let cri_container_config = Self::to_cri_container_config(&config);
        let container_id = self
            .cri
            .create_container(&pod_id, &cri_pod_config, cri_container_config)
            .await?;
        Ok(scop::CreateContainerResponse { container_id })
    }

    async fn start_container(&mut self, container_id: String) -> anyhow::Result<()> {
        self.cri.start_container(&container_id).await
    }

    async fn stop_container(&mut self, container_id: String, timeout: i64) -> anyhow::Result<()> {
        self.cri.stop_container(&container_id, timeout).await
    }

    async fn remove_container(&mut self, container_id: String) -> anyhow::Result<()> {
        self.cri.remove_container(&container_id).await
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    match args.command {
        Command::Daemon {
            node_name,
            plugin_addr,
            containerd_socket,
        } => {
            tracing::info!("SCOC agent starting");
            tracing::info!("  node_name: {}", node_name);
            tracing::info!("  plugin_addr: {}", plugin_addr);
            tracing::info!("  containerd_socket: {}", containerd_socket);

            // Verify CRI connectivity at startup
            {
                let mut cri = CriClient::connect(&containerd_socket).await?;
                let version = cri.version().await?;
                tracing::info!("containerd version: {}", version);
            }

            // Connect to plugin via SCOP with reconnection loop
            let labels = HashMap::new();

            loop {
                // Create fresh CRI client and agent for each connection attempt
                let cri = CriClient::connect(&containerd_socket).await?;
                let agent = CriAgent::new(cri);

                tracing::info!("connecting to plugin at {}", plugin_addr);
                match scop::dial(&plugin_addr, &node_name, labels.clone(), agent).await {
                    Ok(()) => {
                        tracing::info!("connection closed gracefully");
                        break;
                    }
                    Err(e) => {
                        tracing::error!("connection error: {}", e);
                        tracing::info!("reconnecting in 5 seconds...");
                        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    }
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

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
    /// Pod sandbox operations for testing.
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
    /// Create and run a test pod sandbox.
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
    /// Stop a pod sandbox.
    Stop {
        /// Pod sandbox ID.
        #[arg(long)]
        id: String,
        #[arg(long, default_value = "/run/containerd/containerd.sock")]
        containerd_socket: String,
    },
    /// Remove a pod sandbox.
    Remove {
        /// Pod sandbox ID.
        #[arg(long)]
        id: String,
        #[arg(long, default_value = "/run/containerd/containerd.sock")]
        containerd_socket: String,
    },
}

#[derive(Subcommand)]
enum ContainerAction {
    /// Create a container in a pod sandbox.
    Create {
        /// Pod sandbox ID.
        #[arg(long)]
        sandbox_id: String,
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

            // Connect to containerd CRI
            let mut cri = CriClient::connect(&containerd_socket).await?;
            let version = cri.version().await?;
            tracing::info!("containerd version: {}", version);

            // TODO: Connect to plugin via SCOP (Phase 2+)

            // Keep running until interrupted
            tokio::signal::ctrl_c().await?;
            tracing::info!("SCOC agent shutting down");
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
                let sandbox_id = cri.run_pod_sandbox(config).await?;
                println!("{sandbox_id}");
            }
            PodAction::Stop {
                id,
                containerd_socket,
            } => {
                let mut cri = CriClient::connect(&containerd_socket).await?;
                cri.stop_pod_sandbox(&id).await?;
                println!("Pod sandbox stopped");
            }
            PodAction::Remove {
                id,
                containerd_socket,
            } => {
                let mut cri = CriClient::connect(&containerd_socket).await?;
                cri.remove_pod_sandbox(&id).await?;
                println!("Pod sandbox removed");
            }
        },

        Command::Container { action } => match action {
            ContainerAction::Create {
                sandbox_id,
                name,
                image,
                containerd_socket,
            } => {
                let mut cri = CriClient::connect(&containerd_socket).await?;
                // Create a minimal sandbox config for the container creation call
                let sandbox_config = cri::test_pod_config("sandbox", "default");
                let container_config = cri::test_container_config(&name, &image);
                let container_id = cri
                    .create_container(&sandbox_id, &sandbox_config, container_config)
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

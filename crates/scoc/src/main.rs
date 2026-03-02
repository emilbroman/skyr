use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
struct Args {
    #[arg(long)]
    node_name: String,
    #[arg(long)]
    plugin_addr: String,
    #[arg(long, default_value = "/run/containerd/containerd.sock")]
    containerd_socket: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    tracing::info!("SCOC agent starting (skeleton)");
    tracing::info!("  node_name: {}", args.node_name);
    tracing::info!("  plugin_addr: {}", args.plugin_addr);
    tracing::info!("  containerd_socket: {}", args.containerd_socket);

    // Placeholder: actual CRI client implementation comes in Phase 1
    tokio::signal::ctrl_c().await?;
    Ok(())
}

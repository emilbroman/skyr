use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
struct Args {
    #[arg(long)]
    bind: String,
    #[arg(long)]
    ndb_hostname: String,
    #[arg(long)]
    buildkit_addr: String,
    #[arg(long)]
    registry_url: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    tracing::info!("Container plugin starting (skeleton)");
    tracing::info!("  bind: {}", args.bind);
    tracing::info!("  ndb_hostname: {}", args.ndb_hostname);
    tracing::info!("  buildkit_addr: {}", args.buildkit_addr);
    tracing::info!("  registry_url: {}", args.registry_url);

    // Placeholder: actual RTP server implementation comes in Phase 4
    tokio::signal::ctrl_c().await?;
    Ok(())
}

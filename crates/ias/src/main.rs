use clap::Parser;
use tonic::transport::Server;

#[derive(Parser, Debug)]
#[command(name = "ias", about = "Skyr Identity and Access Service")]
struct Cli {
    #[arg(long, default_value = "0.0.0.0")]
    host: String,
    #[arg(long, default_value_t = 50100)]
    port: u16,
    /// The Skyr region this IAS serves (e.g. `stockholm`). Validated as
    /// `[a-z]+`. Stamped as the `issuer_region` claim on every token this
    /// service signs, and returned to other regions as the publisher of
    /// `GetVerifyingKey`.
    #[arg(long)]
    region: String,
    /// Hostname (optionally with `:port`) of the regional UDB Redis. The
    /// `udb` crate prepends `redis://` and appends `/` to form the
    /// connection URL.
    #[arg(long)]
    udb_host: String,
    /// Path to the 32-byte raw Ed25519 secret key used to sign identity
    /// tokens for users whose home region is this one.
    #[arg(long)]
    signing_key: std::path::PathBuf,
    /// Salt mixed into per-username, per-frame challenge derivation. Must
    /// be stable across restarts so that an in-flight challenge survives a
    /// rolling restart of the IAS.
    #[arg(long, env = "SKYR_CHALLENGE_SALT")]
    challenge_salt: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    let region: ids::RegionId = cli
        .region
        .parse()
        .map_err(|e: ids::ParseIdError| anyhow::anyhow!("invalid --region: {e}"))?;

    let signing_identity = udb::SigningIdentity::load(region.clone(), &cli.signing_key)
        .map_err(|e| anyhow::anyhow!("failed to load --signing-key: {e}"))?;

    let udb_client = udb::ClientBuilder::new()
        .known_node(&cli.udb_host)
        .signing_identity(signing_identity)
        .build()
        .await?;

    let challenger = ias::challenge::Challenger::new(cli.challenge_salt.into_bytes());

    let service = ias::service::IasService::new(udb_client, challenger);

    let bind_target = format!("{}:{}", cli.host, cli.port);
    let addr = tokio::net::lookup_host(&bind_target)
        .await?
        .next()
        .ok_or_else(|| anyhow::anyhow!("failed to resolve bind address {bind_target}"))?;
    tracing::info!(%region, "ias listening on {addr}");

    Server::builder()
        .add_service(ias::IdentityAndAccessServer::new(service))
        .serve_with_shutdown(addr, async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;

    Ok(())
}

use std::sync::Arc;

use anyhow::{Context, anyhow};
use clap::Args;
use russh::keys::ssh_key::PrivateKey;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::auth;

#[derive(Args, Debug)]
pub struct PortForwardArgs {
    /// Resource QID (e.g., org/repo::env::Std/Container.Pod.Port:name)
    resource_qid: String,

    /// Local port to bind
    local_port: u16,

    /// SCS server address (host:port)
    #[arg(long, default_value = "skyr.cloud:22")]
    scs_address: String,
}

/// Minimal SSH client handler — accepts any host key.
struct SshHandler;

impl russh::client::Handler for SshHandler {
    type Error = anyhow::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        // Accept any host key (similar to StrictHostKeyChecking=no).
        // In production, this should verify against known hosts.
        Ok(true)
    }
}

pub async fn run_port_forward(args: PortForwardArgs) -> anyhow::Result<()> {
    let user_config = read_user_config().await?;
    let key_path = auth::expand_tilde(&user_config.key)?;
    let private_key_pem = tokio::fs::read_to_string(&key_path)
        .await
        .with_context(|| format!("failed to read private key at {}", key_path.display()))?;
    let private_key = PrivateKey::from_openssh(private_key_pem.as_str())
        .context("failed to parse private key")?;
    let key_with_hash = russh::keys::PrivateKeyWithHashAlg::new(Arc::new(private_key), None);

    // Connect to SCS
    let ssh_config = Arc::new(russh::client::Config::default());
    let mut handle = russh::client::connect(ssh_config, &args.scs_address, SshHandler)
        .await
        .with_context(|| format!("failed to connect to SCS at {}", args.scs_address))?;

    // Authenticate
    let auth_result = handle
        .authenticate_publickey(&user_config.username, key_with_hash)
        .await
        .context("SSH authentication failed")?;

    if auth_result != russh::client::AuthResult::Success {
        return Err(anyhow!(
            "SSH authentication rejected for user '{}'",
            user_config.username
        ));
    }

    // Bind local port
    let listener = TcpListener::bind(format!("127.0.0.1:{}", args.local_port))
        .await
        .with_context(|| format!("failed to bind to port {}", args.local_port))?;

    eprintln!(
        "Forwarding 127.0.0.1:{} -> {}",
        args.local_port, args.resource_qid
    );
    eprintln!("Press Ctrl+C to stop.");

    let resource_qid = args.resource_qid;

    loop {
        tokio::select! {
            accept = listener.accept() => {
                let (tcp_stream, peer_addr) = accept.context("failed to accept connection")?;
                eprintln!("Handling connection from {peer_addr}");

                match proxy_connection(&handle, &resource_qid, tcp_stream).await {
                    Ok(()) => eprintln!("Connection from {peer_addr} closed"),
                    Err(e) => eprintln!("Connection from {peer_addr} error: {e:#}"),
                }
            }
            _ = tokio::signal::ctrl_c() => {
                eprintln!("\nShutting down.");
                return Ok(());
            }
        }
    }
}

/// Proxy a single TCP connection through an SSH channel to SCS.
async fn proxy_connection(
    handle: &russh::client::Handle<SshHandler>,
    resource_qid: &str,
    tcp_stream: tokio::net::TcpStream,
) -> anyhow::Result<()> {
    // Open a session channel
    let mut channel = handle
        .channel_open_session()
        .await
        .context("failed to open SSH session channel")?;

    // Send the port-forward exec command
    let exec_cmd = format!("port-forward {resource_qid}");
    channel
        .exec(true, exec_cmd.as_bytes())
        .await
        .context("failed to send exec request")?;

    // Get writer first (returns 'static handle), then reader (needs &mut)
    let channel_writer = channel.make_writer();
    let mut channel_reader = channel.make_reader();
    let (mut tcp_read, mut tcp_write) = tcp_stream.into_split();

    // Task: local TCP → SSH channel (via writer)
    let tcp_to_ssh = async move {
        let mut writer = tokio::io::BufWriter::new(channel_writer);
        let mut buf = vec![0u8; 32 * 1024];
        loop {
            match tcp_read.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if writer.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                    if writer.flush().await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    };

    // Task: SSH channel → local TCP (via reader)
    let ssh_to_tcp = async move {
        let mut buf = vec![0u8; 32 * 1024];
        loop {
            match channel_reader.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if tcp_write.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    };

    tokio::select! {
        _ = tcp_to_ssh => {}
        _ = ssh_to_tcp => {}
    }

    Ok(())
}

/// User config stored at ~/.config/skyr/user.json
#[derive(serde::Deserialize)]
struct UserConfig {
    username: String,
    key: String,
}

async fn read_user_config() -> anyhow::Result<UserConfig> {
    let home = std::env::var("HOME").context("HOME is not set")?;
    let path = std::path::PathBuf::from(home)
        .join(".config")
        .join("skyr")
        .join("user.json");
    let contents = tokio::fs::read_to_string(&path).await.with_context(|| {
        format!(
            "failed to read user config at {} (have you run `skyr signin`?)",
            path.display()
        )
    })?;
    serde_json::from_str::<UserConfig>(&contents)
        .with_context(|| format!("failed to parse {}", path.display()))
}

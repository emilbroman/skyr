mod auth;
mod git;
mod port_forward;

use std::{collections::BTreeMap, net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::anyhow;
use clap::Parser;
use ids::RepoQid;
use russh::{
    ChannelId, MethodKind,
    keys::{PrivateKey, ssh_key::PublicKey},
    server::{self, Auth, Config, Handler, Server},
};
use tokio::{sync::mpsc, task};
use tracing::Instrument;

use auth::{ensure_repo_access, ensure_repo_exists};
use git::CommandHandler;
use port_forward::handle_port_forward;

#[derive(Parser)]
enum Program {
    Daemon {
        #[clap(short = 'a', long = "address", default_value = "127.0.0.1:22")]
        address: String,

        #[clap(short = 'k', long = "key", default_value = "host.pem")]
        key: PathBuf,

        #[clap(long = "cdb-hostname", default_value = "localhost")]
        cdb_hostname: String,

        #[clap(long = "udb-hostname", default_value = "localhost")]
        udb_hostname: String,

        /// RDB hostname for resource lookups (port-forward).
        #[clap(long = "rdb-hostname", default_value = "localhost")]
        rdb_hostname: String,

        /// Node registry hostname (Redis) for SCOC address lookups (port-forward).
        #[clap(long = "node-registry-hostname", default_value = "localhost")]
        node_registry_hostname: String,

        /// Skyr region this SCS serves (e.g. `stockholm`). Validated as
        /// `[a-z]+`. Used by the RDB client for resource-id construction
        /// when servicing port-forward lookups.
        #[clap(long = "region")]
        region: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    match Program::parse() {
        Program::Daemon {
            address,
            key,
            cdb_hostname,
            udb_hostname,
            rdb_hostname,
            node_registry_hostname,
            region,
        } => {
            let region: ids::RegionId = region
                .parse()
                .map_err(|e: ids::ParseIdError| anyhow::anyhow!("invalid --region: {e}"))?;

            let client = cdb::ClientBuilder::new()
                .known_node(cdb_hostname)
                .build()
                .await?;
            let udb_client = udb::ClientBuilder::new()
                .known_node(udb_hostname)
                .build()
                .await?;
            let rdb_client = rdb::ClientBuilder::new()
                .known_node(rdb_hostname)
                .region(region.clone())
                .build()
                .await?;
            let node_registry_url = format!("redis://{node_registry_hostname}/");
            let node_registry_redis = redis::Client::open(node_registry_url)?
                .get_multiplexed_async_connection()
                .await?;

            tracing::info!("listening on {address}");
            ConfigServer {
                client,
                udb_client,
                rdb_client,
                node_registry_redis,
            }
            .run_on_address(
                Arc::new(Config {
                    methods: (&[MethodKind::PublicKey][..]).into(),
                    keys: vec![PrivateKey::from_openssh(
                        std::fs::read_to_string(&key)
                            .map_err(|e| {
                                anyhow::anyhow!(
                                    "failed to load private key from {}: {}",
                                    key.display(),
                                    e
                                )
                            })?
                            .as_bytes(),
                    )?],
                    ..Default::default()
                }),
                address,
            )
            .await?;
            Ok(())
        }
    }
}

/// Error type for messages that are safe to show to the SSH client.
#[derive(Debug)]
struct UserFacingError(String);

impl std::fmt::Display for UserFacingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UserFacingError {}

struct ConfigServer {
    client: cdb::Client,
    udb_client: udb::Client,
    rdb_client: rdb::Client,
    node_registry_redis: redis::aio::MultiplexedConnection,
}

impl Server for ConfigServer {
    type Handler = ConfigHandler;

    fn new_client(&mut self, peer_addr: Option<SocketAddr>) -> Self::Handler {
        let span = tracing::info_span!("peer", peer = ?peer_addr);
        ConfigHandler {
            span,
            channels: Default::default(),
            user: None,
            client: self.client.clone(),
            udb_client: self.udb_client.clone(),
            rdb_client: self.rdb_client.clone(),
            node_registry_redis: self.node_registry_redis.clone(),
        }
    }
}

#[derive(Parser, Debug)]
enum ChannelCommand {
    #[command(name = "git-receive-pack")]
    ReceivePack {
        #[arg()]
        repo: RepoQid,
    },
    #[command(name = "git-upload-pack")]
    UploadPack {
        #[arg()]
        repo: RepoQid,
    },
    #[command(name = "port-forward")]
    PortForward {
        #[arg()]
        resource_qid: String,
    },
}

/// Messages sent from the SSH handler to per-channel tasks.
enum ChannelMessage {
    /// Initial command parsed from the exec request.
    Command(Result<ChannelCommand, clap::Error>),
    /// Data received on the channel (for port-forward).
    Data(Vec<u8>),
    /// EOF received on the channel.
    Eof,
}

struct ConfigHandler {
    span: tracing::Span,
    channels: BTreeMap<ChannelId, mpsc::Sender<ChannelMessage>>,
    user: Option<udb::User>,
    client: cdb::Client,
    udb_client: udb::Client,
    rdb_client: rdb::Client,
    node_registry_redis: redis::aio::MultiplexedConnection,
}

impl Handler for ConfigHandler {
    type Error = anyhow::Error;

    async fn auth_publickey(
        &mut self,
        username: &str,
        public_key: &PublicKey,
    ) -> Result<Auth, Self::Error> {
        let _guard = self.span.enter();
        let fingerprint = public_key.fingerprint(Default::default()).to_string();
        let user_client = self.udb_client.user(username);
        let user = match user_client.get().await {
            Ok(user) => Some(user),
            Err(udb::UserQueryError::NotFound) => None,
            Err(err) => {
                return Err(anyhow!(
                    "failed to check existence for user {username}: {err}"
                ));
            }
        };

        let Some(user) = user else {
            tracing::info!(username, fingerprint, "rejecting auth for unknown user",);
            return Ok(Auth::Reject {
                proceed_with_methods: None,
                partial_success: false,
            });
        };

        let pubkeys = user_client.pubkeys();
        let fingerprint_allowed = pubkeys
            .contains(&fingerprint)
            .await
            .map_err(|err| anyhow!("failed to check pubkey for user {username}: {err}"))?;

        if !fingerprint_allowed {
            tracing::info!(username, fingerprint, "rejecting auth for unknown pubkey",);
            return Ok(Auth::Reject {
                proceed_with_methods: None,
                partial_success: false,
            });
        }

        tracing::info!(username, fingerprint, "accepted auth");
        self.user = Some(user);
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        mut channel: russh::Channel<server::Msg>,
        _session: &mut server::Session,
    ) -> Result<bool, Self::Error> {
        let (tx, mut rx) = mpsc::channel(32);
        let channel_id = channel.id();
        self.channels.insert(channel_id, tx);
        let span = tracing::info_span!(parent: &self.span, "channel", ch = %u32::from(channel_id));
        let user = self.user.clone();
        let client = self.client.clone();
        let udb_client = self.udb_client.clone();
        let rdb_client = self.rdb_client.clone();
        let node_registry_redis = self.node_registry_redis.clone();
        task::spawn(
            async move {
                // Wait for the command message
                let cmd_msg = rx.recv().await;
                let result: anyhow::Result<()> = match (&user, cmd_msg) {
                    (None, _) => Err(UserFacingError("not authenticated".to_string()).into()),
                    (Some(user), Some(ChannelMessage::Command(Ok(cmd)))) => match cmd {
                        ChannelCommand::ReceivePack { ref repo }
                        | ChannelCommand::UploadPack { ref repo } => {
                            // Git commands read SSH channel data directly via
                            // `channel.make_reader()`; the per-channel mpsc is only
                            // used by port-forward. Drop `rx` so `data()` callbacks'
                            // `tx.send().await` returns immediately (closed receiver)
                            // instead of blocking the russh session loop once the
                            // 32-message buffer fills, which otherwise deadlocks any
                            // push or fetch larger than ~32 SSH data packets.
                            drop(rx);
                            if let Err(err) = ensure_repo_access(user, repo, &udb_client).await {
                                Err(err)
                            } else if let Err(err) = ensure_repo_exists(&client, repo).await {
                                Err(err)
                            } else {
                                let handler = CommandHandler {
                                    _user: user,
                                    channel: &mut channel,
                                    client: client.repo(repo.clone()),
                                };
                                match cmd {
                                    ChannelCommand::ReceivePack { .. } => {
                                        handler.receive_pack().await
                                    }
                                    ChannelCommand::UploadPack { .. } => {
                                        handler.upload_pack().await
                                    }
                                    _ => unreachable!(),
                                }
                            }
                        }
                        ChannelCommand::PortForward { resource_qid } => {
                            handle_port_forward(
                                user,
                                &resource_qid,
                                &mut channel,
                                &mut rx,
                                &rdb_client,
                                &udb_client,
                                node_registry_redis,
                            )
                            .await
                        }
                    },
                    (_, Some(ChannelMessage::Command(Err(e)))) => {
                        Err(UserFacingError(format!("{e}")).into())
                    }
                    (_, _) => Err(UserFacingError("unexpected message".to_string()).into()),
                };

                match result {
                    Ok(()) => {
                        channel.exit_status(0).await.unwrap_or_default();
                    }
                    Err(e) => {
                        tracing::error!("command failed: {e:#}");
                        let client_msg = if e.downcast_ref::<UserFacingError>().is_some() {
                            format!("{e}\n")
                        } else {
                            "internal server error\n".to_string()
                        };
                        channel
                            .extended_data(1, client_msg.as_bytes())
                            .await
                            .unwrap_or_default();
                        channel.exit_status(1).await.unwrap_or_default();
                    }
                }

                channel.eof().await.unwrap_or_default();
                channel.close().await.unwrap_or_default();
            }
            .instrument(span),
        );
        Ok(true)
    }

    async fn channel_close(
        &mut self,
        channel: ChannelId,
        _session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.channels.remove(&channel);
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        _session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        if let Some(tx) = self.channels.get(&channel) {
            let _ = tx.send(ChannelMessage::Data(data.to_vec())).await;
        }
        Ok(())
    }

    async fn channel_eof(
        &mut self,
        channel: ChannelId,
        _session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        if let Some(tx) = self.channels.get(&channel) {
            let _ = tx.send(ChannelMessage::Eof).await;
        }
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        _session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        let mut args =
            comma::parse_command(String::from_utf8_lossy(data).as_ref()).unwrap_or_default();
        args.insert(0, "ssh skyr".into());
        // Strip leading slash from repo path (ssh:// URLs produce paths like "/org/repo")
        if let Some(repo_arg) = args.get_mut(2)
            && let Some(stripped) = repo_arg.strip_prefix('/')
        {
            *repo_arg = stripped.to_string();
        }
        let result = ChannelCommand::try_parse_from(args);
        if let Some(tx) = self.channels.get(&channel) {
            let _ = tx.send(ChannelMessage::Command(result)).await;
        }
        Ok(())
    }
}

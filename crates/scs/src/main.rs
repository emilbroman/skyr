mod auth;
mod git;
mod pools;
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
use pools::{CdbPool, IasPool, NodeRegistryPool, RdbPool};
use port_forward::handle_port_forward;

#[derive(Parser)]
enum Program {
    Daemon {
        #[clap(short = 'a', long = "address", default_value = "127.0.0.1:22")]
        address: String,

        #[clap(short = 'k', long = "key", default_value = "host.pem")]
        key: PathBuf,

        /// Template used to construct region-scoped Skyr peer service
        /// addresses. Substitutes `{service}` (required) and `{region}`
        /// (optional). Defaults to `{service}.{region}.int.skyr.cloud` —
        /// override per stack (e.g. `{service}.<namespace>.svc.cluster.local`
        /// for a single-region Kubernetes deployment).
        ///
        /// SCS does not have its own region — it routes per-channel using
        /// token-equivalent SSH pubkey checks against the user's home
        /// region IAS, GDDB lookups for repos, and the resource's region
        /// (encoded structurally in `ResourceQid`) for port-forward.
        #[clap(long = "service-address-template", default_value_t = ids::ServiceAddressTemplate::default_template())]
        service_address_template: ids::ServiceAddressTemplate,

        /// Region to bootstrap the GDDB ScyllaDB session against. Used
        /// only as the region substituted into `--service-address-template`
        /// for the initial known-node address. GDDB is logically global;
        /// the Scylla session discovers the rest of the cluster from there.
        #[clap(long = "gddb-bootstrap-region")]
        gddb_bootstrap_region: String,
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
            service_address_template,
            gddb_bootstrap_region,
        } => {
            let template = service_address_template;
            let gddb_bootstrap_region: ids::RegionId =
                gddb_bootstrap_region
                    .parse()
                    .map_err(|e: ids::ParseIdError| {
                        anyhow::anyhow!("invalid --gddb-bootstrap-region: {e}")
                    })?;

            let gddb_client = gddb::ClientBuilder::new()
                .known_node(template.format("gddb", &gddb_bootstrap_region))
                .build()
                .await?;
            let ias_pool = IasPool::new(template.clone());
            let cdb_pool = CdbPool::new(template.clone());
            let rdb_pool = RdbPool::new(template.clone());
            let node_registry_pool = NodeRegistryPool::new(template.clone());

            tracing::info!("listening on {address}");
            ConfigServer {
                gddb_client,
                ias_pool,
                cdb_pool,
                rdb_pool,
                node_registry_pool,
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
    gddb_client: gddb::Client,
    ias_pool: IasPool,
    cdb_pool: CdbPool,
    rdb_pool: RdbPool,
    node_registry_pool: NodeRegistryPool,
}

impl Server for ConfigServer {
    type Handler = ConfigHandler;

    fn new_client(&mut self, peer_addr: Option<SocketAddr>) -> Self::Handler {
        let span = tracing::info_span!("peer", peer = ?peer_addr);
        ConfigHandler {
            span,
            channels: Default::default(),
            username: None,
            gddb_client: self.gddb_client.clone(),
            ias_pool: self.ias_pool.clone(),
            cdb_pool: self.cdb_pool.clone(),
            rdb_pool: self.rdb_pool.clone(),
            node_registry_pool: self.node_registry_pool.clone(),
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
    /// Set when SSH pubkey auth succeeds; the verified Skyr username
    /// presented by the SSH client.
    username: Option<String>,
    gddb_client: gddb::Client,
    ias_pool: IasPool,
    cdb_pool: CdbPool,
    rdb_pool: RdbPool,
    node_registry_pool: NodeRegistryPool,
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

        // Resolve the user's home region: usernames are personal-org names
        // in GDDB, so the user's UDB record lives wherever GDDB says the
        // org is homed. An unknown user (no GDDB entry) is rejected the
        // same way an unknown fingerprint is — without leaking which is
        // which.
        let Ok(user_org_id) = username.parse::<ids::OrgId>() else {
            tracing::info!(username, fingerprint, "rejecting auth for invalid username");
            return Ok(Auth::Reject {
                proceed_with_methods: None,
                partial_success: false,
            });
        };

        let home = match self.gddb_client.lookup_org(&user_org_id).await {
            Ok(Some(region)) => region,
            Ok(None) => {
                tracing::info!(username, fingerprint, "rejecting auth for unknown user");
                return Ok(Auth::Reject {
                    proceed_with_methods: None,
                    partial_success: false,
                });
            }
            Err(err) => {
                return Err(anyhow!("failed to look up user {username} in GDDB: {err}"));
            }
        };

        let mut ias = self
            .ias_pool
            .for_region(&home)
            .await
            .map_err(|e| anyhow!("failed to connect to IAS in {home}: {e}"))?;

        let credentials = match ias
            .list_credentials(ias::proto::ListCredentialsRequest {
                username: username.to_string(),
            })
            .await
        {
            Ok(resp) => resp.into_inner().credentials,
            Err(status) if status.code() == tonic::Code::NotFound => {
                tracing::info!(username, fingerprint, "rejecting auth for unknown user");
                return Ok(Auth::Reject {
                    proceed_with_methods: None,
                    partial_success: false,
                });
            }
            Err(status) => {
                return Err(anyhow!(
                    "IAS ListCredentials failed for {username}: {status}"
                ));
            }
        };

        if !credentials.iter().any(|c| c.fingerprint == fingerprint) {
            tracing::info!(username, fingerprint, "rejecting auth for unknown pubkey");
            return Ok(Auth::Reject {
                proceed_with_methods: None,
                partial_success: false,
            });
        }

        tracing::info!(username, fingerprint, "accepted auth");
        self.username = Some(username.to_string());
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
        let username = self.username.clone();
        let gddb_client = self.gddb_client.clone();
        let ias_pool = self.ias_pool.clone();
        let cdb_pool = self.cdb_pool.clone();
        let rdb_pool = self.rdb_pool.clone();
        let node_registry_pool = self.node_registry_pool.clone();
        task::spawn(
            async move {
                // Wait for the command message
                let cmd_msg = rx.recv().await;
                let result: anyhow::Result<()> = match (&username, cmd_msg) {
                    (None, _) => Err(UserFacingError("not authenticated".to_string()).into()),
                    (Some(username), Some(ChannelMessage::Command(Ok(cmd)))) => match cmd {
                        ChannelCommand::ReceivePack { repo } => {
                            // Git commands read SSH channel data directly via
                            // `channel.make_reader()`; the per-channel mpsc is only
                            // used by port-forward. Drop `rx` so `data()` callbacks'
                            // `tx.send().await` returns immediately (closed receiver)
                            // instead of blocking the russh session loop once the
                            // 32-message buffer fills, which otherwise deadlocks any
                            // push or fetch larger than ~32 SSH data packets.
                            drop(rx);
                            handle_repo_command(
                                username,
                                RepoCommandKind::ReceivePack,
                                &repo,
                                &mut channel,
                                &gddb_client,
                                &ias_pool,
                                &cdb_pool,
                            )
                            .await
                        }
                        ChannelCommand::UploadPack { repo } => {
                            drop(rx);
                            handle_repo_command(
                                username,
                                RepoCommandKind::UploadPack,
                                &repo,
                                &mut channel,
                                &gddb_client,
                                &ias_pool,
                                &cdb_pool,
                            )
                            .await
                        }
                        ChannelCommand::PortForward { resource_qid } => {
                            handle_port_forward(
                                username,
                                &resource_qid,
                                &mut channel,
                                &mut rx,
                                &rdb_pool,
                                &gddb_client,
                                &ias_pool,
                                &node_registry_pool,
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

enum RepoCommandKind {
    ReceivePack,
    UploadPack,
}

/// Resolve a git repo command to its home region's CDB and dispatch.
///
/// Repos pin to a single region at creation; GDDB tells us which one.
/// The CDB pool then either reuses an existing connection to that
/// region or opens one lazily.
async fn handle_repo_command(
    username: &str,
    kind: RepoCommandKind,
    repo: &RepoQid,
    channel: &mut russh::Channel<server::Msg>,
    gddb_client: &gddb::Client,
    ias_pool: &IasPool,
    cdb_pool: &CdbPool,
) -> anyhow::Result<()> {
    let home = gddb_client
        .lookup_repo(repo)
        .await
        .map_err(|e| anyhow!("failed to look up repository '{repo}' in GDDB: {e}"))?
        .ok_or_else(|| UserFacingError(format!("repository '{repo}' does not exist")))?;

    ensure_repo_access(username, repo, gddb_client, ias_pool).await?;

    let cdb_client = cdb_pool
        .for_region(&home)
        .await
        .map_err(|e| anyhow!("failed to connect to CDB in {home}: {e}"))?;

    ensure_repo_exists(&cdb_client, repo).await?;

    let handler = CommandHandler {
        _username: username,
        channel,
        client: cdb_client.repo(repo.clone()),
    };
    match kind {
        RepoCommandKind::ReceivePack => handler.receive_pack().await,
        RepoCommandKind::UploadPack => handler.upload_pack().await,
    }
}

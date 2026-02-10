use futures_util::StreamExt;
use gix_pack::data::input::BytesToEntriesIter;
use std::{collections::BTreeMap, net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::{anyhow, bail};
use cdb::RepositoryName;
use clap::Parser;
use gix_protocol::{
    futures_lite::AsyncWriteExt,
    handshake,
    transport::client::{Capabilities, capabilities::Capability},
};
use gix_ref::Reference;
use russh::{
    Channel, ChannelId,
    keys::{PrivateKey, ssh_key::PublicKey},
    server::{self, Auth, Config, Handler, Server},
};
use slog::{Drain, Logger, error, info, o};
use tokio::{io::AsyncReadExt, sync::mpsc, task};
use tokio_util::compat::TokioAsyncWriteCompatExt;

#[derive(Parser)]
enum Program {
    Daemon {
        #[clap(short = 'a', long = "address", default_value = "0.0.0.0:22")]
        address: String,

        #[clap(short = 'k', long = "key", default_value = "host.pem")]
        key: PathBuf,

        #[clap(long = "db-hostname", default_value = "localhost")]
        db_hostname: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let decorator = slog_term::TermDecorator::new().build();
    let drain = slog_term::FullFormat::new(decorator).build().fuse();
    let drain = slog_async::Async::new(drain).build().fuse();

    let log = slog::Logger::root(drain, o!());

    match Program::parse() {
        Program::Daemon {
            address,
            key,
            db_hostname,
        } => {
            let client = cdb::ClientBuilder::new()
                .known_node(db_hostname)
                .build()
                .await?;

            info!(log, "listening on {address}");
            ConfigServer { log, client }
                .run_on_address(
                    Arc::new(Config {
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

struct ConfigServer {
    log: Logger,
    client: cdb::Client,
}

impl Server for ConfigServer {
    type Handler = ConfigHandler;

    fn new_client(&mut self, peer_addr: Option<SocketAddr>) -> Self::Handler {
        ConfigHandler {
            log: self.log.new(o!("peer" => peer_addr)),
            channels: Default::default(),
            user: None,
            client: self.client.clone(),
        }
    }
}

#[derive(Parser, Debug)]
enum ChannelCommand {
    #[command(name = "git-receive-pack")]
    ReceivePack {
        #[arg()]
        repo: RepositoryName,
    },
    #[command(name = "git-upload-pack")]
    UploadPack {
        #[arg()]
        repo: RepositoryName,
    },
}

struct ConfigHandler {
    log: Logger,
    channels: BTreeMap<ChannelId, mpsc::Sender<Result<ChannelCommand, clap::Error>>>,
    user: Option<String>,
    client: cdb::Client,
}

impl Handler for ConfigHandler {
    type Error = anyhow::Error;

    async fn auth_publickey(
        &mut self,
        user: &str,
        public_key: &PublicKey,
    ) -> Result<Auth, Self::Error> {
        info!(
            self.log,
            "TODO: authn {} -- {}",
            user,
            public_key.fingerprint(Default::default())
        );
        self.user = Some(user.to_string());
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        mut channel: russh::Channel<server::Msg>,
        _session: &mut server::Session,
    ) -> Result<bool, Self::Error> {
        let (tx, mut rx) = mpsc::channel(1);
        let channel_id = channel.id();
        self.channels.insert(channel_id, tx);
        let log = self.log.new(o!("ch" => u32::from(channel_id)));
        let user = self.user.clone();
        let client = self.client.clone();
        task::spawn(async move {
            loop {
                let client = client.clone();
                let result: anyhow::Result<()> = match (&user, rx.recv().await) {
                    (None, _) => Err(anyhow!("not authenticated")),
                    (Some(user), Some(Ok(ChannelCommand::ReceivePack { repo }))) => {
                        CommandHandler {
                            log: log.clone(),
                            user,
                            channel: &mut channel,
                            client: client.repo(repo),
                        }
                        .receive_pack()
                        .await
                    }
                    (Some(user), Some(Ok(ChannelCommand::UploadPack { repo }))) => {
                        CommandHandler {
                            log: log.clone(),
                            user,
                            channel: &mut channel,
                            client: client.repo(repo),
                        }
                        .upload_pack()
                        .await
                    }
                    (_, Some(Err(e))) => Err(e.into()),
                    (_, None) => break,
                };

                match result {
                    Ok(()) => {
                        channel.exit_status(0).await.unwrap_or_default();
                    }
                    Err(e) => {
                        channel
                            .extended_data(1, e.to_string().as_bytes())
                            .await
                            .unwrap_or_default();
                        channel.exit_status(1).await.unwrap_or_default();
                    }
                }
            }

            channel.close().await.unwrap_or_default();
        });
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

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        _session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        let mut args =
            comma::parse_command(String::from_utf8_lossy(data).as_ref()).unwrap_or(vec![]);
        args.insert(0, "ssh skyr".into());
        let result = ChannelCommand::try_parse_from(args);
        if let Some(tx) = self.channels.remove(&channel) {
            tx.send(result).await.unwrap_or_default();
        }
        Ok(())
    }
}

struct CommandHandler<'a> {
    log: Logger,
    channel: &'a mut Channel<server::Msg>,
    user: &'a str,
    client: cdb::RepositoryClient,
}

impl<'a> CommandHandler<'a> {
    async fn upload_pack(self) -> anyhow::Result<()> {
        todo!()
    }

    async fn advertise_refs(
        &self,
        server_caps: &[u8],
        mut refs: impl futures::Stream<Item = Reference> + Unpin,
    ) -> anyhow::Result<()> {
        let mut server_caps = Some(server_caps);

        let mut pkt =
            gix_packetline::async_io::Writer::new(self.channel.make_writer().compat_write());

        while let Some(reference) = refs.next().await {
            if let gix_ref::Target::Object(oid) = reference.target {
                let mut line = vec![];
                oid.write_hex_to(&mut line)?;
                line.push(b' ');
                line.extend_from_slice(reference.name.as_bstr().as_ref());
                if let Some(caps) = server_caps.take() {
                    line.push(b'\0');
                    line.extend_from_slice(caps);
                }
                line.push(b'\n');
                pkt.write_all(&line).await?;
            }
        }

        if let Some(caps) = server_caps.take() {
            let mut line = vec![];
            gix_hash::ObjectId::Sha1(Default::default()).write_hex_to(&mut line)?;
            line.extend_from_slice(b" capabilities^{}\0");
            line.extend_from_slice(caps);
            line.push(b'\n');
            pkt.write_all(&line).await?;
        }

        pkt.flush().await?;

        Ok(())
    }

    async fn receive_pack(self) -> anyhow::Result<()> {
        self.advertise_refs(b"report-status", futures_util::stream::empty())
            .await?;

        let mut r = self.channel.make_reader();
        let mut pack = [0u8; 4];
        if let Err(e) = r.read_exact(&mut pack).await {
            error!(self.log, "{e}");
            return Ok(());
        }

        if &pack != b"PACK" {
            bail!("invalid packfile header");
        }

        let iter = BytesToEntriesIter::new_from_header(
            std::io::BufReader::new(tokio_util::io::SyncIoBridge::new(&mut r)),
            gix_pack::data::input::Mode::Verify,
            gix_pack::data::input::EntryDataMode::Keep,
            gix_hash::Kind::Sha1,
        )?;

        for entry in iter {
            let entry = entry?;

            // INCOMPLETE

            self.client.write_object(id, object).await?;
        }

        Ok(())
    }
}

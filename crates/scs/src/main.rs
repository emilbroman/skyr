use futures::StreamExt;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
};

use anyhow::{anyhow, bail};
use cdb::DeploymentState;
use clap::Parser;
use futures::AsyncWriteExt;
use gix_ref::Reference;
use ids::{EnvironmentId, RepoQid};
use russh::{
    Channel, ChannelId, MethodKind,
    keys::{PrivateKey, ssh_key::PublicKey},
    server::{self, Auth, Config, Handler, Server},
};
use tokio::{io::AsyncReadExt, sync::mpsc, task};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tracing::Instrument;

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
        } => {
            let client = cdb::ClientBuilder::new()
                .known_node(cdb_hostname)
                .build()
                .await?;
            let udb_client = udb::ClientBuilder::new()
                .known_node(udb_hostname)
                .build()
                .await?;

            tracing::info!("listening on {address}");
            ConfigServer { client, udb_client }
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

struct ConfigServer {
    client: cdb::Client,
    udb_client: udb::Client,
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
}

struct ConfigHandler {
    span: tracing::Span,
    channels: BTreeMap<ChannelId, mpsc::Sender<Result<ChannelCommand, clap::Error>>>,
    user: Option<udb::User>,
    client: cdb::Client,
    udb_client: udb::Client,
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
        let mut user_client = self.udb_client.user(username);
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

        let mut pubkeys = user_client.pubkeys();
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
        let (tx, mut rx) = mpsc::channel(1);
        let channel_id = channel.id();
        self.channels.insert(channel_id, tx);
        let span = tracing::info_span!(parent: &self.span, "channel", ch = %u32::from(channel_id));
        let user = self.user.clone();
        let client = self.client.clone();
        task::spawn(
            async move {
                loop {
                    let client = client.clone();
                    let result: anyhow::Result<()> = match (&user, rx.recv().await) {
                        (None, _) => Err(anyhow!("not authenticated")),
                        (Some(user), Some(Ok(cmd))) => {
                            let repo = match &cmd {
                                ChannelCommand::ReceivePack { repo }
                                | ChannelCommand::UploadPack { repo } => repo,
                            };
                            if let Err(err) = ensure_repo_access(user, repo) {
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
                                }
                            }
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
                                .extended_data(1, format!("{e}\n").as_bytes())
                                .await
                                .unwrap_or_default();
                            channel.exit_status(1).await.unwrap_or_default();
                        }
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

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        _session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        let mut args =
            comma::parse_command(String::from_utf8_lossy(data).as_ref()).unwrap_or(vec![]);
        args.insert(0, "ssh skyr".into());
        // Strip leading slash from repo path (ssh:// URLs produce paths like "/org/repo")
        if let Some(repo_arg) = args.get_mut(2)
            && let Some(stripped) = repo_arg.strip_prefix('/')
        {
            *repo_arg = stripped.to_string();
        }
        let result = ChannelCommand::try_parse_from(args);
        if let Some(tx) = self.channels.remove(&channel) {
            tx.send(result).await.unwrap_or_default();
        }
        Ok(())
    }
}

fn ensure_repo_access(user: &udb::User, repo: &RepoQid) -> anyhow::Result<()> {
    if repo.org.as_str() != user.username {
        bail!(
            "permission denied: user '{}' cannot access repository '{}'; expected organization '{}'",
            user.username,
            repo,
            user.username
        );
    }

    Ok(())
}

async fn ensure_repo_exists(client: &cdb::Client, repo: &RepoQid) -> anyhow::Result<()> {
    match client.repo(repo.clone()).get().await {
        Ok(_) => Ok(()),
        Err(cdb::RepositoryQueryError::NotFound) => {
            bail!("repository '{}' does not exist", repo);
        }
        Err(err) => Err(anyhow!("failed to query repository '{}': {}", repo, err)),
    }
}

struct CommandHandler<'a> {
    channel: &'a mut Channel<server::Msg>,
    _user: &'a udb::User,
    client: cdb::RepositoryClient,
}

impl<'a> CommandHandler<'a> {
    async fn upload_pack(self) -> anyhow::Result<()> {
        let refs = self.collect_refs().await?;

        self.advertise_refs(b"side-band-64k", futures::stream::iter(refs))
            .await?;

        // Create writer before reader: make_writer returns a 'static handle (clones the
        // internal sender), so it can coexist with the reader's &mut borrow on the channel.
        let mut out = self.channel.make_writer().compat_write();
        let mut r = self.channel.make_reader();
        let mut use_sideband = false;
        let wants = {
            let mut pkt = gix_packetline::async_io::StreamingPeekableIter::new(
                r.compat(),
                &[gix_packetline::PacketLineRef::Flush],
                false,
            );
            let mut wants = Vec::new();
            let mut first_want = true;
            while let Some(line) = pkt.read_line().await {
                let line = line??;
                let gix_packetline::PacketLineRef::Data(data) = line else {
                    continue;
                };
                let mut data: &[u8] = data;
                if let Some(trimmed) = data.strip_suffix(b"\n") {
                    data = trimmed;
                }
                if let Some(nul_pos) = data.iter().position(|&b| b == 0) {
                    if first_want {
                        let caps = &data[nul_pos + 1..];
                        if caps.split(|b| *b == b' ').any(|c| c == b"side-band-64k") {
                            use_sideband = true;
                        }
                    }
                    data = &data[..nul_pos];
                }
                if data.starts_with(b"want ") {
                    let mut parts = data[5..].split(|b| *b == b' ');
                    let hex = parts.next().unwrap_or_default();
                    if first_want {
                        if parts.any(|p| p == b"side-band-64k") {
                            use_sideband = true;
                        }
                        first_want = false;
                    }
                    let oid = gix_hash::ObjectId::from_hex(hex)?;
                    wants.push(oid);
                }
            }
            r = pkt.into_inner().into_inner();
            wants
        };

        if wants.is_empty() {
            return Ok(());
        }

        // Have/done negotiation: collect have OIDs and respond with NAK after
        // each flush so the client can continue sending more haves.
        let haves: HashSet<gix_hash::ObjectId> = {
            let mut haves = HashSet::new();
            let mut pkt = gix_packetline::async_io::StreamingPeekableIter::new(
                r.compat(),
                &[gix_packetline::PacketLineRef::Flush],
                false,
            );
            loop {
                let mut got_done = false;
                while let Some(line) = pkt.read_line().await {
                    let line = line??;
                    if let gix_packetline::PacketLineRef::Data(data) = line {
                        let data = data.strip_suffix(b"\n").unwrap_or(data);
                        if data == b"done" {
                            got_done = true;
                            break;
                        }
                        if let Some(hex) = data.strip_prefix(b"have ")
                            && let Ok(oid) = gix_hash::ObjectId::from_hex(hex)
                        {
                            haves.insert(oid);
                        }
                    }
                }
                if got_done {
                    break;
                }
                // Client sent a flush after a batch of haves — respond with NAK
                // so it knows to continue sending more haves (or send done).
                gix_packetline::async_io::encode::write_packet_line(
                    &gix_packetline::PacketLineRef::Data(b"NAK\n"),
                    &mut out,
                )
                .await?;
                out.flush().await?;
                pkt.reset();
            }
            haves
        };

        struct PackObject {
            kind: gix_object::Kind,
            data: Vec<u8>,
        }

        fn encode_pack_header(kind: gix_object::Kind, mut size: u64) -> Vec<u8> {
            let type_id = match kind {
                gix_object::Kind::Commit => 1u8,
                gix_object::Kind::Tree => 2u8,
                gix_object::Kind::Blob => 3u8,
                gix_object::Kind::Tag => 4u8,
            };
            let mut out = Vec::new();
            let mut byte = (type_id << 4) | ((size as u8) & 0x0f);
            size >>= 4;
            if size > 0 {
                byte |= 0x80;
            }
            out.push(byte);
            while size > 0 {
                let mut b = (size as u8) & 0x7f;
                size >>= 7;
                if size > 0 {
                    b |= 0x80;
                }
                out.push(b);
            }
            out
        }

        /// Walk the object graph from `wants`, stopping at any OID the client
        /// already has (the `haves` set). This makes incremental fetches send
        /// only new objects instead of the entire repository.
        async fn collect_objects(
            client: &cdb::RepositoryClient,
            wants: Vec<gix_hash::ObjectId>,
            haves: &HashSet<gix_hash::ObjectId>,
        ) -> anyhow::Result<Vec<PackObject>> {
            let mut stack = wants;
            let mut seen: HashSet<gix_hash::ObjectId> = HashSet::new();
            let mut out = Vec::new();

            while let Some(oid) = stack.pop() {
                if !seen.insert(oid) {
                    continue;
                }
                if haves.contains(&oid) {
                    continue;
                }
                let (kind, data) = client.read_raw_object(oid).await?;

                match kind {
                    gix_object::Kind::Commit => {
                        let mut iter = gix_object::CommitRefIter::from_bytes(&data);
                        let tree = iter.tree_id()?;
                        stack.push(tree);
                        for parent in iter.parent_ids() {
                            stack.push(parent);
                        }
                    }
                    gix_object::Kind::Tree => {
                        for entry in gix_object::TreeRefIter::from_bytes(&data) {
                            let entry = entry?;
                            stack.push(entry.oid.to_owned());
                        }
                    }
                    gix_object::Kind::Tag => {
                        let target = gix_object::TagRefIter::from_bytes(&data).target_id()?;
                        stack.push(target);
                    }
                    gix_object::Kind::Blob => {}
                }

                out.push(PackObject { kind, data });
            }

            Ok(out)
        }

        let objects = collect_objects(&self.client, wants, &haves).await?;

        let mut pack = Vec::new();
        pack.extend_from_slice(b"PACK");
        pack.extend_from_slice(&2u32.to_be_bytes());
        pack.extend_from_slice(&(objects.len() as u32).to_be_bytes());

        for obj in objects {
            pack.extend_from_slice(&encode_pack_header(obj.kind, obj.data.len() as u64));
            let mut compressor = gix_features::zlib::stream::deflate::Write::new(Vec::new());
            use std::io::Write as _;
            compressor.write_all(&obj.data)?;
            compressor.flush()?;
            let compressed = compressor.into_inner();
            pack.extend_from_slice(&compressed);
        }

        let mut hasher = gix_hash::hasher(gix_hash::Kind::Sha1);
        hasher.update(&pack);
        let digest = hasher.try_finalize()?;
        pack.extend_from_slice(digest.as_slice());

        gix_packetline::async_io::encode::write_packet_line(
            &gix_packetline::PacketLineRef::Data(b"NAK\n"),
            &mut out,
        )
        .await?;
        if use_sideband {
            for chunk in pack.chunks(65515) {
                let mut sb = vec![1u8];
                sb.extend_from_slice(chunk);
                gix_packetline::async_io::encode::write_packet_line(
                    &gix_packetline::PacketLineRef::Data(&sb),
                    &mut out,
                )
                .await?;
            }
            gix_packetline::async_io::encode::write_packet_line(
                &gix_packetline::PacketLineRef::Flush,
                &mut out,
            )
            .await?;
        } else {
            out.write_all(&pack).await?;
        }
        out.flush().await?;

        Ok(())
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

        gix_packetline::async_io::encode::write_packet_line(
            &gix_packetline::PacketLineRef::Flush,
            pkt.inner_mut(),
        )
        .await?;
        pkt.flush().await?;

        Ok(())
    }

    async fn receive_pack(self) -> anyhow::Result<()> {
        let refs = self.collect_refs().await?;

        self.advertise_refs(
            b"report-status delete-refs side-band-64k ofs-delta",
            futures::stream::iter(refs),
        )
        .await?;

        let mut r = self.channel.make_reader();

        struct RefUpdate {
            old: gix_hash::ObjectId,
            new: gix_hash::ObjectId,
            name: String,
        }

        let null_oid = gix_hash::Kind::Sha1.null();
        let mut updates = Vec::new();

        let mut use_sideband = false;
        let mut client_wants_report = false;
        {
            let mut pkt = gix_packetline::async_io::StreamingPeekableIter::new(
                r.compat(),
                &[gix_packetline::PacketLineRef::Flush],
                false,
            );

            let mut first_command = true;
            while let Some(line) = pkt.read_line().await {
                let line = line??;
                let gix_packetline::PacketLineRef::Data(data) = line else {
                    continue;
                };

                let mut data: &[u8] = data;
                if let Some(trimmed) = data.strip_suffix(b"\n") {
                    data = trimmed;
                }

                if let Some(nul_pos) = data.iter().position(|&b| b == 0) {
                    if first_command {
                        let caps = &data[nul_pos + 1..];
                        for cap in caps.split(|b| *b == b' ') {
                            if cap == b"side-band-64k" {
                                use_sideband = true;
                            } else if cap == b"report-status" {
                                client_wants_report = true;
                            }
                        }
                        first_command = false;
                    }
                    data = &data[..nul_pos];
                }

                if data.is_empty() {
                    continue;
                }

                if data.starts_with(b"shallow ") || data.starts_with(b"unshallow ") {
                    continue;
                }

                let mut parts = data.splitn(3, |b| *b == b' ');
                let Some(old_hex) = parts.next() else {
                    continue;
                };
                let Some(new_hex) = parts.next() else {
                    continue;
                };
                let Some(name) = parts.next() else {
                    continue;
                };

                let old = gix_hash::ObjectId::from_hex(old_hex)?;
                let new = gix_hash::ObjectId::from_hex(new_hex)?;
                let name = String::from_utf8(name.to_vec())?;

                updates.push(RefUpdate { old, new, name });
            }

            r = pkt.into_inner().into_inner();
        }

        struct RawEntry {
            pack_offset: u64,
            header: gix_pack::data::entry::Header,
            data: Option<Vec<u8>>,
        }

        fn decode_varint(data: &[u8], mut idx: usize) -> anyhow::Result<(u64, usize)> {
            let mut size = 0u64;
            let mut shift = 0u32;
            let start = idx;
            loop {
                let byte = *data
                    .get(idx)
                    .ok_or_else(|| anyhow!("delta header truncated"))?;
                idx += 1;
                size |= u64::from(byte & 0x7f) << shift;
                if byte & 0x80 == 0 {
                    break;
                }
                shift += 7;
            }
            Ok((size, idx - start))
        }

        fn apply_delta(base: &[u8], delta: &[u8]) -> anyhow::Result<Vec<u8>> {
            let (base_size, mut consumed) = decode_varint(delta, 0)?;
            let (result_size, result_consumed) = decode_varint(delta, consumed)?;
            consumed += result_consumed;
            if base_size as usize != base.len() {
                bail!(
                    "delta base size mismatch: expected {}, got {}",
                    base_size,
                    base.len()
                );
            }
            let mut out = Vec::with_capacity(result_size as usize);
            while consumed < delta.len() {
                let cmd = delta[consumed];
                consumed += 1;
                if cmd & 0x80 != 0 {
                    let mut ofs: u32 = 0;
                    let mut size: u32 = 0;
                    if cmd & 0x01 != 0 {
                        ofs |= u32::from(delta[consumed]);
                        consumed += 1;
                    }
                    if cmd & 0x02 != 0 {
                        ofs |= u32::from(delta[consumed]) << 8;
                        consumed += 1;
                    }
                    if cmd & 0x04 != 0 {
                        ofs |= u32::from(delta[consumed]) << 16;
                        consumed += 1;
                    }
                    if cmd & 0x08 != 0 {
                        ofs |= u32::from(delta[consumed]) << 24;
                        consumed += 1;
                    }
                    if cmd & 0x10 != 0 {
                        size |= u32::from(delta[consumed]);
                        consumed += 1;
                    }
                    if cmd & 0x20 != 0 {
                        size |= u32::from(delta[consumed]) << 8;
                        consumed += 1;
                    }
                    if cmd & 0x40 != 0 {
                        size |= u32::from(delta[consumed]) << 16;
                        consumed += 1;
                    }
                    if size == 0 {
                        size = 0x10000;
                    }
                    let ofs = ofs as usize;
                    let size = size as usize;
                    let end = ofs
                        .checked_add(size)
                        .ok_or_else(|| anyhow!("delta copy overflow"))?;
                    let slice = base
                        .get(ofs..end)
                        .ok_or_else(|| anyhow!("delta copy out of bounds"))?;
                    out.extend_from_slice(slice);
                } else if cmd == 0 {
                    bail!("delta instruction had unsupported opcode 0");
                } else {
                    let size = cmd as usize;
                    let end = consumed
                        .checked_add(size)
                        .ok_or_else(|| anyhow!("delta insert overflow"))?;
                    let slice = delta
                        .get(consumed..end)
                        .ok_or_else(|| anyhow!("delta insert out of bounds"))?;
                    out.extend_from_slice(slice);
                    consumed = end;
                }
            }
            if out.len() != result_size as usize {
                bail!(
                    "delta result size mismatch: expected {}, got {}",
                    result_size,
                    out.len()
                );
            }
            Ok(out)
        }

        struct CountingReader<'a, R> {
            inner: &'a mut R,
            bytes_read: u64,
        }

        impl<'a, R> CountingReader<'a, R>
        where
            R: tokio::io::AsyncRead + Unpin,
        {
            async fn read_exact(&mut self, buf: &mut [u8]) -> anyhow::Result<()> {
                self.inner.read_exact(buf).await?;
                self.bytes_read += buf.len() as u64;
                Ok(())
            }

            async fn read_byte(&mut self) -> anyhow::Result<u8> {
                let mut b = [0u8; 1];
                self.read_exact(&mut b).await?;
                Ok(b[0])
            }
        }

        async fn read_u32_be<R: tokio::io::AsyncRead + Unpin>(
            r: &mut CountingReader<'_, R>,
        ) -> anyhow::Result<u32> {
            let mut buf = [0u8; 4];
            r.read_exact(&mut buf).await?;
            Ok(u32::from_be_bytes(buf))
        }

        async fn read_leb64<R: tokio::io::AsyncRead + Unpin>(
            r: &mut CountingReader<'_, R>,
        ) -> anyhow::Result<u64> {
            let mut byte = r.read_byte().await?;
            let mut value = u64::from(byte) & 0x7f;
            while byte & 0x80 != 0 {
                byte = r.read_byte().await?;
                value += 1;
                value = (value << 7) + (u64::from(byte) & 0x7f);
            }
            Ok(value)
        }

        async fn read_zlib_object<R: tokio::io::AsyncRead + Unpin>(
            r: &mut CountingReader<'_, R>,
            size: u64,
        ) -> anyhow::Result<Vec<u8>> {
            let size: usize = size
                .try_into()
                .map_err(|_| anyhow!("object too large to fit into memory"))?;
            let mut out = vec![0u8; size];
            let mut written = 0usize;
            let mut decompressor = gix_features::zlib::Decompress::new();
            let mut input = [0u8; 1];
            let mut have_input = false;
            loop {
                if !have_input {
                    r.read_exact(&mut input).await?;
                    have_input = true;
                }
                let before_in = decompressor.total_in();
                let before_out = decompressor.total_out();
                let status = decompressor.decompress(
                    &input,
                    &mut out[written..],
                    gix_features::zlib::FlushDecompress::None,
                )?;
                let consumed = (decompressor.total_in() - before_in) as usize;
                let produced = (decompressor.total_out() - before_out) as usize;
                if consumed > 0 || produced == 0 {
                    have_input = false;
                }
                written += produced;
                if written > out.len() {
                    bail!(
                        "decompression overflow: expected {} bytes, got {}",
                        out.len(),
                        written
                    );
                }
                if status == gix_features::zlib::Status::StreamEnd {
                    break;
                }
            }
            if written != out.len() {
                bail!(
                    "decompression incomplete: expected {} bytes, got {}",
                    out.len(),
                    written
                );
            }
            Ok(out)
        }

        let mut entries = Vec::new();

        if updates.iter().any(|u| u.new != null_oid) {
            let mut reader = CountingReader {
                inner: &mut r,
                bytes_read: 0,
            };

            let mut header = [0u8; 4];
            reader.read_exact(&mut header).await?;
            if &header != b"PACK" {
                bail!("invalid packfile header");
            }
            let _version = read_u32_be(&mut reader).await?;
            let num_objects = read_u32_be(&mut reader).await?;

            entries = Vec::with_capacity(num_objects as usize);
            for _ in 0..num_objects {
                let pack_offset = reader.bytes_read;
                let mut c = reader.read_byte().await?;
                let type_id = (c >> 4) & 0b0000_0111;
                let mut size = u64::from(c) & 0b0000_1111;
                let mut shift = 4u32;
                while c & 0b1000_0000 != 0 {
                    c = reader.read_byte().await?;
                    size += u64::from(c & 0b0111_1111) << shift;
                    shift += 7;
                }
                let header = match type_id {
                    1 => gix_pack::data::entry::Header::Commit,
                    2 => gix_pack::data::entry::Header::Tree,
                    3 => gix_pack::data::entry::Header::Blob,
                    4 => gix_pack::data::entry::Header::Tag,
                    6 => gix_pack::data::entry::Header::OfsDelta {
                        base_distance: read_leb64(&mut reader).await?,
                    },
                    7 => {
                        let mut base = gix_hash::Kind::Sha1.null();
                        reader.read_exact(base.as_mut_slice()).await?;
                        gix_pack::data::entry::Header::RefDelta { base_id: base }
                    }
                    other => bail!("unsupported pack object type {other}"),
                };

                let data = read_zlib_object(&mut reader, size).await?;
                entries.push(RawEntry {
                    pack_offset,
                    header,
                    data: Some(data),
                });
            }

            let mut trailer = [0u8; 20];
            reader.read_exact(&mut trailer).await?;
        }

        let mut oid_by_offset: HashMap<u64, gix_hash::ObjectId> = HashMap::new();

        loop {
            let mut progress = false;
            for entry in &mut entries {
                let pack_offset = entry.pack_offset;
                if oid_by_offset.contains_key(&pack_offset) {
                    continue;
                }
                match entry.header {
                    gix_pack::data::entry::Header::Commit
                    | gix_pack::data::entry::Header::Tree
                    | gix_pack::data::entry::Header::Blob
                    | gix_pack::data::entry::Header::Tag => {
                        let kind = entry.header.as_kind().expect("base objects have a kind");
                        let data = entry
                            .data
                            .take()
                            .ok_or_else(|| anyhow!("missing base object data"))?;
                        let id = gix_object::compute_hash(gix_hash::Kind::Sha1, kind, &data)?;
                        let object =
                            gix_object::ObjectRef::from_bytes(kind, &data)?.into_owned()?;
                        tracing::debug!("writing {} {}", object.kind(), id);
                        self.client.write_object(id, object).await?;
                        oid_by_offset.insert(pack_offset, id);
                        progress = true;
                    }
                    gix_pack::data::entry::Header::OfsDelta { base_distance } => {
                        let base_offset = pack_offset
                            .checked_sub(base_distance)
                            .ok_or_else(|| anyhow!("ofs-delta base offset underflow"))?;
                        let Some(base_id) = oid_by_offset.get(&base_offset).copied() else {
                            continue;
                        };
                        let (kind, base_data) =
                            self.client.read_raw_object(base_id).await.map_err(|err| {
                                anyhow!("failed to load ofs-delta base {}: {}", base_id, err)
                            })?;
                        let delta = entry
                            .data
                            .take()
                            .ok_or_else(|| anyhow!("missing delta data"))?;
                        let data = apply_delta(&base_data, &delta)?;
                        let id = gix_object::compute_hash(gix_hash::Kind::Sha1, kind, &data)?;
                        let object =
                            gix_object::ObjectRef::from_bytes(kind, &data)?.into_owned()?;
                        tracing::debug!("writing {} {}", object.kind(), id);
                        self.client.write_object(id, object).await?;
                        oid_by_offset.insert(pack_offset, id);
                        progress = true;
                    }
                    gix_pack::data::entry::Header::RefDelta { base_id } => {
                        let (kind, base_data) = match self.client.read_raw_object(base_id).await {
                            Ok(result) => result,
                            Err(cdb::LoadObjectError::NotFound) => continue,
                            Err(err) => {
                                return Err(anyhow!(
                                    "failed to load ref-delta base object {}: {}",
                                    base_id,
                                    err
                                ));
                            }
                        };

                        let delta = entry
                            .data
                            .take()
                            .ok_or_else(|| anyhow!("missing delta data"))?;
                        let data = apply_delta(&base_data, &delta)?;
                        let id = gix_object::compute_hash(gix_hash::Kind::Sha1, kind, &data)?;
                        let object =
                            gix_object::ObjectRef::from_bytes(kind, &data)?.into_owned()?;
                        tracing::debug!("writing {} {}", object.kind(), id);
                        self.client.write_object(id, object).await?;
                        oid_by_offset.insert(pack_offset, id);
                        progress = true;
                    }
                }
            }
            if oid_by_offset.len() == entries.len() {
                break;
            }
            if !progress {
                bail!("unable to resolve all deltas in pack");
            }
        }

        let mut results = Vec::new();
        for update in updates.iter() {
            let environment_id = EnvironmentId::from_git_ref(&update.name)
                .map_err(|e| anyhow!("invalid git ref '{}': {}", update.name, e))?;

            if update.new != null_oid {
                let deployment_id = ids::DeploymentId::from_bytes(update.new.as_bytes())
                    .map_err(|e| anyhow!("invalid deployment id: {}", e))?;
                let deployment = self
                    .client
                    .deployment(environment_id.clone(), deployment_id);
                deployment.set(DeploymentState::Desired).await?;
            }

            if update.old != null_oid && update.old != update.new {
                let state = if update.new == null_oid {
                    DeploymentState::Undesired
                } else {
                    DeploymentState::Lingering
                };
                let old_deployment_id = ids::DeploymentId::from_bytes(update.old.as_bytes())
                    .map_err(|e| anyhow!("invalid deployment id: {}", e))?;
                let deployment = self.client.deployment(environment_id, old_deployment_id);
                let new_deployment_id = ids::DeploymentId::from_bytes(update.new.as_bytes())
                    .map_err(|e| anyhow!("invalid deployment id: {}", e))?;
                let (r1, r2) = futures::join!(
                    deployment.set(state),
                    deployment.mark_superseded_by(&new_deployment_id),
                );
                r1?;
                r2?;
            }

            results.push(update.name.clone());
        }

        drop(r);

        if client_wants_report {
            if use_sideband {
                let mut out = self.channel.make_writer().compat_write();
                // Build report-status as pkt-line encoded bytes
                let mut report = Vec::new();
                {
                    use std::io::Write as _;
                    let line = b"unpack ok\n";
                    write!(report, "{:04x}", 4 + line.len())?;
                    report.extend_from_slice(line);
                    for name in &results {
                        let line = format!("ok {name}\n");
                        write!(report, "{:04x}", 4 + line.len())?;
                        report.extend_from_slice(line.as_bytes());
                    }
                    report.extend_from_slice(b"0000");
                }
                // Send report through side-band channel 1
                for chunk in report.chunks(65515) {
                    let mut sb = vec![1u8];
                    sb.extend_from_slice(chunk);
                    gix_packetline::async_io::encode::write_packet_line(
                        &gix_packetline::PacketLineRef::Data(&sb),
                        &mut out,
                    )
                    .await?;
                }
                gix_packetline::async_io::encode::write_packet_line(
                    &gix_packetline::PacketLineRef::Flush,
                    &mut out,
                )
                .await?;
                out.flush().await?;
            } else {
                let mut pkt = gix_packetline::async_io::Writer::new(
                    self.channel.make_writer().compat_write(),
                );
                pkt.write_all(b"unpack ok\n").await?;
                for name in &results {
                    let line = format!("ok {name}\n");
                    pkt.write_all(line.as_bytes()).await?;
                }
                gix_packetline::async_io::encode::write_packet_line(
                    &gix_packetline::PacketLineRef::Flush,
                    pkt.inner_mut(),
                )
                .await?;
                pkt.flush().await?;
            }
        }

        Ok(())
    }

    async fn collect_refs(&self) -> anyhow::Result<Vec<Reference>> {
        let mut refs = vec![];

        let mut deployments = self.client.active_deployments().await?;
        while let Some(deployment) = deployments.next().await {
            let deployment = deployment?;

            if matches!(
                deployment.state,
                DeploymentState::Undesired | DeploymentState::Lingering
            ) {
                continue;
            }

            let git_ref = deployment.environment.to_git_ref();
            let commit_oid =
                gix_hash::ObjectId::from_hex(deployment.deployment.as_str().as_bytes())?;
            refs.push(Reference {
                name: gix_ref::FullName::try_from(git_ref.as_str())?,
                target: gix_ref::Target::Object(commit_oid),
                peeled: None,
            });
        }

        Ok(refs)
    }
}

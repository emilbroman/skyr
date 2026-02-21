use futures_util::StreamExt;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
};

use anyhow::{anyhow, bail};
use cdb::{DeploymentId, DeploymentState, RepositoryName};
use clap::Parser;
use gix_protocol::futures_lite::AsyncWriteExt;
use gix_ref::Reference;
use russh::{
    Channel, ChannelId,
    keys::{PrivateKey, ssh_key::PublicKey},
    server::{self, Auth, Config, Handler, Server},
};
use slog::{Drain, Logger, debug, info, o};
use tokio::{io::AsyncReadExt, sync::mpsc, task};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

#[derive(Parser)]
enum Program {
    Daemon {
        #[clap(short = 'a', long = "address", default_value = "0.0.0.0:22")]
        address: String,

        #[clap(short = 'k', long = "key", default_value = "host.pem")]
        key: PathBuf,

        #[clap(long = "db-hostname", default_value = "localhost")]
        db_hostname: String,

        #[clap(long = "udb-hostname", default_value = "localhost")]
        udb_hostname: String,
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
            udb_hostname,
        } => {
            let client = cdb::ClientBuilder::new()
                .known_node(db_hostname)
                .build()
                .await?;
            let udb_client = udb::ClientBuilder::new()
                .known_node(udb_hostname)
                .build()
                .await?;

            info!(log, "listening on {address}");
            ConfigServer {
                log,
                client,
                udb_client,
            }
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
    udb_client: udb::Client,
}

impl Server for ConfigServer {
    type Handler = ConfigHandler;

    fn new_client(&mut self, peer_addr: Option<SocketAddr>) -> Self::Handler {
        ConfigHandler {
            log: self.log.new(o!("peer" => peer_addr)),
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
            info!(
                self.log,
                "rejecting auth for unknown user"; "username" => username, "fingerprint" => fingerprint
            );
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
            info!(
                self.log,
                "rejecting auth for unknown pubkey"; "username" => username, "fingerprint" => fingerprint
            );
            return Ok(Auth::Reject {
                proceed_with_methods: None,
                partial_success: false,
            });
        }

        info!(self.log, "accepted auth"; "username" => username, "fingerprint" => fingerprint);
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
                            _user: user,
                            channel: &mut channel,
                            client: client.repo(repo),
                        }
                        .receive_pack()
                        .await
                    }
                    (Some(user), Some(Ok(ChannelCommand::UploadPack { repo }))) => {
                        CommandHandler {
                            log: log.clone(),
                            _user: user,
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
    _user: &'a udb::User,
    client: cdb::RepositoryClient,
}

impl<'a> CommandHandler<'a> {
    async fn upload_pack(self) -> anyhow::Result<()> {
        let refs = self.collect_refs().await?;

        self.advertise_refs(b"", futures_util::stream::iter(refs.into_iter()))
            .await?;

        let mut r = self.channel.make_reader();
        let wants = {
            let mut pkt = gix_packetline::async_io::StreamingPeekableIter::new(
                r.compat(),
                &[gix_packetline::PacketLineRef::Flush],
                false,
            );
            let mut wants = Vec::new();
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
                    data = &data[..nul_pos];
                }
                if data.starts_with(b"want ") {
                    let hex = data[5..].split(|b| *b == b' ').next().unwrap_or_default();
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

        {
            let mut pkt = gix_packetline::async_io::StreamingPeekableIter::new(
                r.compat(),
                &[gix_packetline::PacketLineRef::Flush],
                false,
            );
            while let Some(line) = pkt.read_line().await {
                let line = line??;
                if let gix_packetline::PacketLineRef::Data(data) = line {
                    let mut data: &[u8] = data;
                    if let Some(trimmed) = data.strip_suffix(b"\n") {
                        data = trimmed;
                    }
                    if data == b"done" {
                        break;
                    }
                }
            }
            r = pkt.into_inner().into_inner();
        }

        #[derive(Clone)]
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

        async fn collect_objects(
            client: &cdb::RepositoryClient,
            wants: Vec<gix_hash::ObjectId>,
        ) -> anyhow::Result<Vec<PackObject>> {
            let mut stack = wants;
            let mut seen: HashSet<gix_hash::ObjectId> = HashSet::new();
            let mut out = Vec::new();

            while let Some(oid) = stack.pop() {
                if !seen.insert(oid) {
                    continue;
                }
                let raw = client.read_raw_object(oid).await?;
                let (kind, size, offset) = gix_object::decode::loose_header(&raw)?;
                let size: usize = size
                    .try_into()
                    .map_err(|_| anyhow!("object too large to fit into memory"))?;
                let body = raw
                    .get(offset..)
                    .and_then(|s| s.get(..size))
                    .ok_or_else(|| anyhow!("object body truncated"))?;

                out.push(PackObject {
                    kind,
                    data: body.to_vec(),
                });

                match kind {
                    gix_object::Kind::Commit => {
                        let mut iter = gix_object::CommitRefIter::from_bytes(body);
                        let tree = iter.tree_id()?;
                        stack.push(tree);
                        for parent in iter.parent_ids() {
                            stack.push(parent);
                        }
                    }
                    gix_object::Kind::Tree => {
                        for entry in gix_object::TreeRefIter::from_bytes(body) {
                            let entry = entry?;
                            stack.push(entry.oid.to_owned());
                        }
                    }
                    gix_object::Kind::Tag => {
                        let target = gix_object::TagRefIter::from_bytes(body).target_id()?;
                        stack.push(target);
                    }
                    gix_object::Kind::Blob => {}
                }
            }

            Ok(out)
        }

        let objects = collect_objects(&self.client, wants).await?;

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

        drop(r);

        let mut out = self.channel.make_writer().compat_write();
        gix_packetline::async_io::encode::write_packet_line(
            &gix_packetline::PacketLineRef::Data(b"NAK\n"),
            &mut out,
        )
        .await?;
        out.write_all(&pack).await?;
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
            b"report-status delete-refs",
            futures_util::stream::iter(refs.into_iter()),
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

        {
            let mut pkt = gix_packetline::async_io::StreamingPeekableIter::new(
                r.compat(),
                &[gix_packetline::PacketLineRef::Flush],
                false,
            );

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

        struct ResolvedObject {
            kind: gix_object::Kind,
            data: Vec<u8>,
            id: gix_hash::ObjectId,
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
                if consumed > 0 {
                    have_input = false;
                } else if produced == 0 {
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

        let mut resolved_by_offset: HashMap<u64, ResolvedObject> = HashMap::new();
        let mut resolved_by_id: HashMap<gix_hash::ObjectId, u64> = HashMap::new();

        loop {
            let mut progress = false;
            for entry in entries.iter_mut() {
                if resolved_by_offset.contains_key(&entry.pack_offset) {
                    continue;
                }
                match entry.header {
                    header @ gix_pack::data::entry::Header::Commit
                    | header @ gix_pack::data::entry::Header::Tree
                    | header @ gix_pack::data::entry::Header::Blob
                    | header @ gix_pack::data::entry::Header::Tag => {
                        let kind = header.as_kind().expect("base objects have a kind");
                        let data = entry
                            .data
                            .take()
                            .ok_or_else(|| anyhow!("missing base object data"))?;
                        let id = gix_object::compute_hash(gix_hash::Kind::Sha1, kind, &data)?;
                        resolved_by_id.insert(id, entry.pack_offset);
                        resolved_by_offset
                            .insert(entry.pack_offset, ResolvedObject { kind, data, id });
                        progress = true;
                    }
                    gix_pack::data::entry::Header::OfsDelta { base_distance } => {
                        let base_offset = entry
                            .pack_offset
                            .checked_sub(base_distance)
                            .ok_or_else(|| anyhow!("ofs-delta base offset underflow"))?;
                        let delta = entry
                            .data
                            .take()
                            .ok_or_else(|| anyhow!("missing delta data"))?;
                        let (data, kind) = {
                            let Some(base) = resolved_by_offset.get(&base_offset) else {
                                continue;
                            };
                            let data = apply_delta(&base.data, &delta)?;
                            (data, base.kind)
                        };
                        let id = gix_object::compute_hash(gix_hash::Kind::Sha1, kind, &data)?;
                        resolved_by_id.insert(id, entry.pack_offset);
                        resolved_by_offset
                            .insert(entry.pack_offset, ResolvedObject { kind, data, id });
                        progress = true;
                    }
                    gix_pack::data::entry::Header::RefDelta { base_id } => {
                        let Some(base_offset) = resolved_by_id.get(&base_id).copied() else {
                            continue;
                        };
                        let delta = entry
                            .data
                            .take()
                            .ok_or_else(|| anyhow!("missing delta data"))?;
                        let (data, kind) = {
                            let base = resolved_by_offset
                                .get(&base_offset)
                                .ok_or_else(|| anyhow!("ref-delta base not resolved"))?;
                            let data = apply_delta(&base.data, &delta)?;
                            (data, base.kind)
                        };
                        let id = gix_object::compute_hash(gix_hash::Kind::Sha1, kind, &data)?;
                        resolved_by_id.insert(id, entry.pack_offset);
                        resolved_by_offset
                            .insert(entry.pack_offset, ResolvedObject { kind, data, id });
                        progress = true;
                    }
                }
            }
            if resolved_by_offset.len() == entries.len() {
                break;
            }
            if !progress {
                bail!("unable to resolve all deltas in pack");
            }
        }

        if !entries.is_empty() {
            debug!(self.log, "unpacked packfile");

            for entry in entries.iter() {
                let resolved = resolved_by_offset
                    .get(&entry.pack_offset)
                    .ok_or_else(|| anyhow!("missing resolved object for pack entry"))?;
                let object = gix_object::ObjectRef::from_bytes(resolved.kind, &resolved.data)?
                    .into_owned()?;
                debug!(self.log, "writing {} {}", object.kind(), resolved.id);
                self.client.write_object(resolved.id, object).await?;
            }
        }

        let mut results = Vec::new();
        for update in updates.iter() {
            if update.new != null_oid {
                let deployment = self.client.deployment(DeploymentId {
                    ref_name: update.name.clone(),
                    commit_hash: update.new,
                });
                deployment.set(DeploymentState::Desired).await?;
            }

            if update.old != null_oid && update.old != update.new {
                let state = if update.new == null_oid {
                    DeploymentState::Undesired
                } else {
                    DeploymentState::Lingering
                };
                let deployment = self.client.deployment(DeploymentId {
                    ref_name: update.name.clone(),
                    commit_hash: update.old,
                });
                let (r1, r2) = futures::join!(
                    deployment.set(state),
                    deployment.mark_superceded_by(update.new),
                );
                r1?;
                r2?;
            }

            results.push((update.name.clone(), None::<String>));
        }

        drop(r);

        let mut pkt =
            gix_packetline::async_io::Writer::new(self.channel.make_writer().compat_write());
        pkt.write_all(b"unpack ok\n").await?;
        for (name, err) in results {
            let line = match err {
                Some(err) => format!("ng {name} {err}\n"),
                None => format!("ok {name}\n"),
            };
            pkt.write_all(line.as_bytes()).await?;
        }
        gix_packetline::async_io::encode::write_packet_line(
            &gix_packetline::PacketLineRef::Flush,
            pkt.inner_mut(),
        )
        .await?;
        pkt.flush().await?;

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

            refs.push(Reference {
                name: gix_ref::FullName::try_from(deployment.id.ref_name.as_str())?,
                target: gix_ref::Target::Object(deployment.id.commit_hash),
                peeled: None,
            });
        }

        Ok(refs)
    }
}

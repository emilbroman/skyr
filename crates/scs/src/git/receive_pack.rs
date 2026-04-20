use std::collections::HashMap;

use anyhow::{anyhow, bail};
use futures::AsyncWriteExt;
use tokio::io::AsyncReadExt;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use cdb::DeploymentState;
use ids::EnvironmentId;

use super::CommandHandler;

struct RefUpdate {
    old: gix_hash::ObjectId,
    new: gix_hash::ObjectId,
    name: String,
}

struct RawEntry {
    pack_offset: u64,
    header: gix_pack::data::entry::Header,
    data: Option<Vec<u8>>,
}

/// Maximum number of bytes a single LEB128 varint may occupy (10 bytes
/// covers the full u64 range: ceil(64/7) = 10).
const MAX_VARINT_BYTES: usize = 10;

/// Maximum allowed size for a single git object (256 MiB). Prevents
/// user-controlled sizes from causing excessive memory allocation.
const MAX_OBJECT_SIZE: u64 = 256 * 1024 * 1024;

fn decode_varint(data: &[u8], mut idx: usize) -> anyhow::Result<(u64, usize)> {
    let mut size = 0u64;
    let mut shift = 0u32;
    let start = idx;
    loop {
        if idx - start >= MAX_VARINT_BYTES {
            bail!("varint exceeds maximum length");
        }
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
    if result_size > MAX_OBJECT_SIZE {
        bail!(
            "delta result size {} exceeds maximum allowed object size",
            result_size
        );
    }
    let mut out = Vec::with_capacity(result_size as usize);

    /// Read the next byte from `delta` at `consumed`, advancing the index.
    fn next_delta_byte(delta: &[u8], consumed: &mut usize) -> anyhow::Result<u8> {
        let byte = *delta
            .get(*consumed)
            .ok_or_else(|| anyhow!("delta copy command truncated"))?;
        *consumed += 1;
        Ok(byte)
    }

    while consumed < delta.len() {
        let cmd = delta[consumed];
        consumed += 1;
        if cmd & 0x80 != 0 {
            let mut ofs: u32 = 0;
            let mut size: u32 = 0;
            if cmd & 0x01 != 0 {
                ofs |= u32::from(next_delta_byte(delta, &mut consumed)?);
            }
            if cmd & 0x02 != 0 {
                ofs |= u32::from(next_delta_byte(delta, &mut consumed)?) << 8;
            }
            if cmd & 0x04 != 0 {
                ofs |= u32::from(next_delta_byte(delta, &mut consumed)?) << 16;
            }
            if cmd & 0x08 != 0 {
                ofs |= u32::from(next_delta_byte(delta, &mut consumed)?) << 24;
            }
            if cmd & 0x10 != 0 {
                size |= u32::from(next_delta_byte(delta, &mut consumed)?);
            }
            if cmd & 0x20 != 0 {
                size |= u32::from(next_delta_byte(delta, &mut consumed)?) << 8;
            }
            if cmd & 0x40 != 0 {
                size |= u32::from(next_delta_byte(delta, &mut consumed)?) << 16;
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
    hasher: gix_hash::Hasher,
}

impl<'a, R> CountingReader<'a, R>
where
    R: tokio::io::AsyncRead + Unpin,
{
    async fn read_exact(&mut self, buf: &mut [u8]) -> anyhow::Result<()> {
        self.inner.read_exact(buf).await?;
        self.bytes_read += buf.len() as u64;
        self.hasher.update(buf);
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
    let mut bytes_read = 1usize;
    while byte & 0x80 != 0 {
        if bytes_read >= MAX_VARINT_BYTES {
            bail!("LEB128 varint exceeds maximum length");
        }
        byte = r.read_byte().await?;
        bytes_read += 1;
        value += 1;
        value = (value << 7) + (u64::from(byte) & 0x7f);
    }
    Ok(value)
}

async fn read_zlib_object<R: tokio::io::AsyncRead + Unpin>(
    r: &mut CountingReader<'_, R>,
    size: u64,
) -> anyhow::Result<Vec<u8>> {
    if size > MAX_OBJECT_SIZE {
        bail!("object size {} exceeds maximum allowed object size", size);
    }
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

impl<'a> CommandHandler<'a> {
    pub(crate) async fn receive_pack(self) -> anyhow::Result<()> {
        let refs = self.collect_refs().await?;

        self.advertise_refs(
            b"report-status delete-refs side-band-64k ofs-delta",
            futures::stream::iter(refs),
        )
        .await?;

        let mut r = self.channel.make_reader();

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

        let mut entries = Vec::new();

        if updates.iter().any(|u| u.new != null_oid) {
            let mut reader = CountingReader {
                inner: &mut r,
                bytes_read: 0,
                hasher: gix_hash::hasher(gix_hash::Kind::Sha1),
            };

            let mut header = [0u8; 4];
            reader.read_exact(&mut header).await?;
            if &header != b"PACK" {
                bail!("invalid packfile header");
            }
            let _version = read_u32_be(&mut reader).await?;
            let num_objects = read_u32_be(&mut reader).await?;

            /// Maximum number of objects allowed in a single packfile.
            const MAX_PACK_OBJECTS: u32 = 1_000_000;
            if num_objects > MAX_PACK_OBJECTS {
                bail!(
                    "packfile contains {} objects, exceeding the limit of {}",
                    num_objects,
                    MAX_PACK_OBJECTS
                );
            }

            entries = Vec::with_capacity(num_objects as usize);
            for _ in 0..num_objects {
                let pack_offset = reader.bytes_read;
                let mut c = reader.read_byte().await?;
                let type_id = (c >> 4) & 0b0000_0111;
                let mut size = u64::from(c) & 0b0000_1111;
                let mut shift = 4u32;
                let mut header_bytes = 1usize;
                while c & 0b1000_0000 != 0 {
                    if header_bytes >= MAX_VARINT_BYTES {
                        bail!("pack object size varint exceeds maximum length");
                    }
                    c = reader.read_byte().await?;
                    header_bytes += 1;
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

            let expected_checksum = reader
                .hasher
                .try_finalize()
                .map_err(|e| anyhow!("failed to finalize pack checksum: {}", e))?;
            let mut trailer = [0u8; 20];
            // Read trailer directly from inner reader (not through the hasher)
            reader.inner.read_exact(&mut trailer).await?;
            if trailer != expected_checksum.as_slice() {
                bail!("packfile SHA-1 checksum mismatch");
            }
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
}

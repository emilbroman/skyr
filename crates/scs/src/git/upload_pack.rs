use std::collections::HashSet;

use futures::AsyncWriteExt;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use super::CommandHandler;

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
///
/// When `max_depth` is `Some(n)`, commit traversal stops at depth `n`
/// from the wanted commits (depth 1 = the want itself). Trees and blobs
/// reachable from included commits are always sent regardless of depth.
async fn collect_objects(
    client: &cdb::RepositoryClient,
    wants: Vec<gix_hash::ObjectId>,
    haves: &HashSet<gix_hash::ObjectId>,
    max_depth: Option<u32>,
) -> anyhow::Result<Vec<PackObject>> {
    let mut stack: Vec<(gix_hash::ObjectId, u32)> = wants.into_iter().map(|oid| (oid, 1)).collect();
    let mut seen: HashSet<gix_hash::ObjectId> = HashSet::new();
    let mut out = Vec::new();

    while let Some((oid, depth)) = stack.pop() {
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
                stack.push((tree, depth));
                if max_depth.is_none() || depth < max_depth.unwrap() {
                    for parent in iter.parent_ids() {
                        stack.push((parent, depth + 1));
                    }
                }
            }
            gix_object::Kind::Tree => {
                for entry in gix_object::TreeRefIter::from_bytes(&data) {
                    let entry = entry?;
                    stack.push((entry.oid.to_owned(), depth));
                }
            }
            gix_object::Kind::Tag => {
                let target = gix_object::TagRefIter::from_bytes(&data).target_id()?;
                stack.push((target, depth));
            }
            gix_object::Kind::Blob => {}
        }

        out.push(PackObject { kind, data });
    }

    Ok(out)
}

impl<'a> CommandHandler<'a> {
    pub(crate) async fn upload_pack(self) -> anyhow::Result<()> {
        let refs = self.collect_refs().await?;

        self.advertise_refs(
            b"multi_ack_detailed no-done side-band-64k shallow",
            futures::stream::iter(refs),
        )
        .await?;

        // Create writer before reader: make_writer returns a 'static handle (clones the
        // internal sender), so it can coexist with the reader's &mut borrow on the channel.
        let mut out = self.channel.make_writer().compat_write();
        let mut r = self.channel.make_reader();
        let mut use_sideband = false;
        let mut use_multi_ack_detailed = false;
        let mut use_no_done = false;
        let mut client_shallow: HashSet<gix_hash::ObjectId> = HashSet::new();
        let mut deepen: Option<u32> = None;
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
                        for cap in caps.split(|b| *b == b' ') {
                            match cap {
                                b"side-band-64k" => use_sideband = true,
                                b"multi_ack_detailed" => use_multi_ack_detailed = true,
                                b"no-done" => use_no_done = true,
                                _ => {}
                            }
                        }
                    }
                    data = &data[..nul_pos];
                }
                if data.starts_with(b"want ") {
                    let mut parts = data[5..].split(|b| *b == b' ');
                    let hex = parts.next().unwrap_or_default();
                    if first_want {
                        for cap in parts {
                            match cap {
                                b"side-band-64k" => use_sideband = true,
                                b"multi_ack_detailed" => use_multi_ack_detailed = true,
                                b"no-done" => use_no_done = true,
                                _ => {}
                            }
                        }
                        first_want = false;
                    }
                    let oid = gix_hash::ObjectId::from_hex(hex)?;
                    wants.push(oid);
                } else if let Some(hex) = data.strip_prefix(b"shallow ") {
                    let oid = gix_hash::ObjectId::from_hex(hex)?;
                    client_shallow.insert(oid);
                } else if let Some(n) = data.strip_prefix(b"deepen ") {
                    deepen = Some(std::str::from_utf8(n)?.parse::<u32>()?);
                }
            }
            r = pkt.into_inner().into_inner();
            wants
        };

        if wants.is_empty() {
            return Ok(());
        }

        // Shallow boundary computation: when the client requests a depth limit
        // (deepen) or reports existing shallow commits, walk the commit graph to
        // determine which commits become shallow (parents excluded) and which
        // become unshallowed (parents now included after deepening).
        if deepen.is_some() || !client_shallow.is_empty() {
            let max_depth = deepen.unwrap_or(u32::MAX);
            let mut new_shallow: Vec<gix_hash::ObjectId> = Vec::new();
            let mut new_unshallow: Vec<gix_hash::ObjectId> = Vec::new();
            {
                let mut seen: HashSet<gix_hash::ObjectId> = HashSet::new();
                let mut stack: Vec<(gix_hash::ObjectId, u32)> =
                    wants.iter().copied().map(|oid| (oid, 1)).collect();
                while let Some((oid, depth)) = stack.pop() {
                    if !seen.insert(oid) {
                        continue;
                    }
                    let Ok((kind, data)) = self.client.read_raw_object(oid).await else {
                        continue;
                    };
                    if kind != gix_object::Kind::Commit {
                        continue;
                    }
                    if depth >= max_depth {
                        new_shallow.push(oid);
                    } else {
                        if client_shallow.contains(&oid) {
                            new_unshallow.push(oid);
                        }
                        let iter = gix_object::CommitRefIter::from_bytes(&data);
                        for parent in iter.parent_ids() {
                            stack.push((parent, depth + 1));
                        }
                    }
                }
            }

            for oid in &new_shallow {
                let line = format!("shallow {oid}\n");
                gix_packetline::async_io::encode::write_packet_line(
                    &gix_packetline::PacketLineRef::Data(line.as_bytes()),
                    &mut out,
                )
                .await?;
            }
            for oid in &new_unshallow {
                let line = format!("unshallow {oid}\n");
                gix_packetline::async_io::encode::write_packet_line(
                    &gix_packetline::PacketLineRef::Data(line.as_bytes()),
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
        }

        // Have/done negotiation: collect have OIDs and — when multi_ack_detailed
        // is active — tell the client which objects we share so it can send an
        // optimal set of haves and know when to stop.
        let (haves, last_common) = {
            let mut haves = HashSet::new();
            let mut last_common: Option<gix_hash::ObjectId> = None;
            let mut sent_ready = false;
            let mut pkt = gix_packetline::async_io::StreamingPeekableIter::new(
                r.compat(),
                &[gix_packetline::PacketLineRef::Flush],
                false,
            );
            loop {
                let mut got_done = false;
                let mut acked_in_batch = false;
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
                            if use_multi_ack_detailed
                                && self.client.read_raw_object(oid).await.is_ok()
                            {
                                last_common = Some(oid);
                                acked_in_batch = true;
                                let ack = format!("ACK {oid} common\n");
                                gix_packetline::async_io::encode::write_packet_line(
                                    &gix_packetline::PacketLineRef::Data(ack.as_bytes()),
                                    &mut out,
                                )
                                .await?;
                            }
                        }
                    }
                }
                if got_done {
                    break;
                }
                // After a flush: if multi_ack_detailed and we found common
                // objects, signal readiness; otherwise send NAK.
                if use_multi_ack_detailed {
                    if let Some(common) = last_common
                        && !sent_ready
                    {
                        sent_ready = true;
                        let ack = format!("ACK {common} ready\n");
                        gix_packetline::async_io::encode::write_packet_line(
                            &gix_packetline::PacketLineRef::Data(ack.as_bytes()),
                            &mut out,
                        )
                        .await?;
                    }
                    if !acked_in_batch {
                        gix_packetline::async_io::encode::write_packet_line(
                            &gix_packetline::PacketLineRef::Data(b"NAK\n"),
                            &mut out,
                        )
                        .await?;
                    }
                } else {
                    gix_packetline::async_io::encode::write_packet_line(
                        &gix_packetline::PacketLineRef::Data(b"NAK\n"),
                        &mut out,
                    )
                    .await?;
                }
                out.flush().await?;
                // If no-done is negotiated and we've sent ready, the client
                // will not send "done" — proceed to packfile generation.
                if use_no_done && sent_ready {
                    break;
                }
                pkt.reset();
            }
            (haves, last_common)
        };

        let objects = collect_objects(&self.client, wants, &haves, deepen).await?;

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

        if let Some(common) = last_common {
            let ack = format!("ACK {common}\n");
            gix_packetline::async_io::encode::write_packet_line(
                &gix_packetline::PacketLineRef::Data(ack.as_bytes()),
                &mut out,
            )
            .await?;
        } else {
            gix_packetline::async_io::encode::write_packet_line(
                &gix_packetline::PacketLineRef::Data(b"NAK\n"),
                &mut out,
            )
            .await?;
        }
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
}

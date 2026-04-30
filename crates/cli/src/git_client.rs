//! Minimal git client for fetching packages from the Skyr SSH server.
//!
//! Uses `russh` for SSH transport and manually implements the git pack
//! protocol (v1) to perform shallow fetches of specific commits. The
//! received packfile objects are unpacked and the commit's tree is
//! extracted to the destination directory.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, anyhow, bail};
use russh::keys::ssh_key::PrivateKey;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::auth;

/// A git client that communicates over SSH with the Skyr git server.
pub struct GitClient {
    ssh_address: String,
    username: String,
    private_key: Arc<PrivateKey>,
}

/// Minimal SSH client handler — accepts any host key.
struct SshHandler;

impl russh::client::Handler for SshHandler {
    type Error = anyhow::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

impl GitClient {
    /// Create a new git client from the user's stored config.
    pub async fn from_config(ssh_address: String) -> anyhow::Result<Self> {
        let user_config = auth::read_user_config()
            .await
            .context("failed to read user config (have you run `skyr signin`?)")?;
        let key_path = auth::expand_tilde(&user_config.key)?;
        let private_key_pem = tokio::fs::read_to_string(&key_path)
            .await
            .with_context(|| format!("failed to read private key at {}", key_path.display()))?;
        let private_key = PrivateKey::from_openssh(private_key_pem.as_str())
            .context("failed to parse private key")?;

        Ok(Self {
            ssh_address,
            username: user_config.username,
            private_key: Arc::new(private_key),
        })
    }

    /// Resolve a specifier to a concrete commit hash by querying the remote refs.
    ///
    /// - `Specifier::Hash` -> returned directly (no network).
    /// - `Specifier::Branch(name)` -> looks up `refs/heads/<name>`.
    /// - `Specifier::Tag(name)` -> looks up `refs/tags/<name>`.
    pub async fn resolve_ref(
        &self,
        repo_qid: &ids::RepoQid,
        specifier: &sclc::Specifier,
    ) -> anyhow::Result<String> {
        match specifier {
            sclc::Specifier::Hash(hex) => Ok(hex.clone()),
            sclc::Specifier::Branch(name) => {
                let target_ref = format!("refs/heads/{name}");
                self.lookup_ref(repo_qid, &target_ref).await
            }
            sclc::Specifier::Tag(name) => {
                let target_ref = format!("refs/tags/{name}");
                self.lookup_ref(repo_qid, &target_ref).await
            }
        }
    }

    /// Fetch a specific commit and extract its tree to `dest_dir`.
    pub async fn fetch_tree_at_commit(
        &self,
        repo_qid: &ids::RepoQid,
        commit_hash: &str,
        dest_dir: &Path,
    ) -> anyhow::Result<()> {
        let mut channel = self.open_upload_pack_channel(repo_qid).await?;

        // Writer must be created before reader: make_writer() returns a
        // 'static handle (clones the internal sender), so it can coexist
        // with the reader's &mut borrow on the channel.
        let writer = channel.make_writer();
        let mut writer = tokio::io::BufWriter::new(writer);
        let mut reader = channel.make_reader();

        // 1. Read and discard ref advertisement.
        read_until_flush(&mut reader).await?;

        // 2. Send want + deepen + done.
        let want_line = format!("want {commit_hash} side-band-64k\n");
        write_pkt_line(&mut writer, want_line.as_bytes()).await?;
        write_flush(&mut writer).await?;
        write_pkt_line(&mut writer, b"deepen 1\n").await?;
        write_flush(&mut writer).await?;
        write_pkt_line(&mut writer, b"done\n").await?;
        writer.flush().await?;

        // 3. Read response: shallow lines, NAK/ACK, then packfile.
        let pack_data = read_sideband_pack(&mut reader).await?;

        // 4. Parse pack and extract tree.
        let objects = parse_pack(&pack_data)?;
        extract_tree(&objects, commit_hash, dest_dir)?;

        Ok(())
    }

    async fn open_upload_pack_channel(
        &self,
        repo_qid: &ids::RepoQid,
    ) -> anyhow::Result<russh::Channel<russh::client::Msg>> {
        let key_with_hash =
            russh::keys::PrivateKeyWithHashAlg::new(Arc::clone(&self.private_key), None);

        let ssh_config = Arc::new(russh::client::Config::default());
        let mut handle = russh::client::connect(ssh_config, &self.ssh_address, SshHandler)
            .await
            .with_context(|| format!("failed to connect to {}", self.ssh_address))?;

        let auth_result = handle
            .authenticate_publickey(&self.username, key_with_hash)
            .await
            .context("SSH authentication failed")?;

        if auth_result != russh::client::AuthResult::Success {
            bail!("SSH authentication rejected for user '{}'", self.username);
        }

        let channel = handle
            .channel_open_session()
            .await
            .context("failed to open SSH session channel")?;

        let exec_cmd = format!("git-upload-pack {}/{}", repo_qid.org, repo_qid.repo);
        channel
            .exec(true, exec_cmd.as_bytes())
            .await
            .context("failed to send exec request")?;

        Ok(channel)
    }

    /// Look up a single ref from the remote's ref advertisement.
    async fn lookup_ref(
        &self,
        repo_qid: &ids::RepoQid,
        target_ref: &str,
    ) -> anyhow::Result<String> {
        let mut channel = self.open_upload_pack_channel(repo_qid).await?;
        let mut reader = channel.make_reader();

        let refs = parse_ref_advertisement(&mut reader).await?;
        refs.get(target_ref)
            .cloned()
            .ok_or_else(|| anyhow!("ref '{target_ref}' not found on remote"))
    }
}

// ---------------------------------------------------------------------------
// Git pack protocol helpers
// ---------------------------------------------------------------------------

/// Read a single pkt-line from the reader.
///
/// Returns `None` for flush packets (0000), `Some(data)` otherwise.
async fn read_pkt_line(reader: &mut (impl AsyncRead + Unpin)) -> anyhow::Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len_str = std::str::from_utf8(&len_buf)?;
    let len = u16::from_str_radix(len_str, 16)? as usize;

    if len == 0 {
        return Ok(None); // flush
    }
    if len < 4 {
        bail!("invalid pkt-line length: {len}");
    }

    let data_len = len - 4;
    let mut data = vec![0u8; data_len];
    reader.read_exact(&mut data).await?;
    Ok(Some(data))
}

/// Write a pkt-line (length-prefixed data).
async fn write_pkt_line(writer: &mut (impl AsyncWrite + Unpin), data: &[u8]) -> anyhow::Result<()> {
    let len = data.len() + 4;
    let header = format!("{len:04x}");
    writer.write_all(header.as_bytes()).await?;
    writer.write_all(data).await?;
    Ok(())
}

/// Write a flush packet (0000).
async fn write_flush(writer: &mut (impl AsyncWrite + Unpin)) -> anyhow::Result<()> {
    writer.write_all(b"0000").await?;
    Ok(())
}

/// Read pkt-lines until a flush, discarding the contents.
async fn read_until_flush(reader: &mut (impl AsyncRead + Unpin)) -> anyhow::Result<()> {
    loop {
        match read_pkt_line(reader).await? {
            None => return Ok(()),
            Some(_) => continue,
        }
    }
}

/// Parse the ref advertisement and return a map of refname -> hex OID.
async fn parse_ref_advertisement(
    reader: &mut (impl AsyncRead + Unpin),
) -> anyhow::Result<HashMap<String, String>> {
    let mut refs = HashMap::new();
    loop {
        let line = match read_pkt_line(reader).await? {
            None => break,
            Some(data) => data,
        };

        // Strip trailing newline.
        let line = if line.last() == Some(&b'\n') {
            &line[..line.len() - 1]
        } else {
            &line
        };

        // Strip capabilities after NUL.
        let line = if let Some(nul) = line.iter().position(|&b| b == 0) {
            &line[..nul]
        } else {
            line
        };

        // Format: "<hex-oid> <refname>"
        if line.len() < 42 {
            continue;
        }
        let hex = std::str::from_utf8(&line[..40])?;
        let refname = std::str::from_utf8(&line[41..])?;
        refs.insert(refname.to_string(), hex.to_string());
    }
    Ok(refs)
}

/// Read the server response after negotiation, demuxing side-band-64k
/// to extract the raw pack data.
async fn read_sideband_pack(reader: &mut (impl AsyncRead + Unpin)) -> anyhow::Result<Vec<u8>> {
    let mut pack_data = Vec::new();

    loop {
        let line = match read_pkt_line(reader).await? {
            None => break,
            Some(data) => data,
        };

        if line.is_empty() {
            continue;
        }

        // Check for non-sideband NAK/ACK lines (plain text).
        if line.starts_with(b"NAK") || line.starts_with(b"ACK") {
            continue;
        }

        // Check for shallow/unshallow notifications.
        if line.starts_with(b"shallow ") || line.starts_with(b"unshallow ") {
            continue;
        }

        // Side-band-64k: first byte is the band number.
        match line[0] {
            1 => {
                // Band 1: pack data.
                pack_data.extend_from_slice(&line[1..]);
            }
            2 => {
                // Band 2: progress messages — ignore.
            }
            3 => {
                // Band 3: error.
                let msg = String::from_utf8_lossy(&line[1..]);
                bail!("remote error: {msg}");
            }
            _ => {
                // Not using sideband — the entire line is pack data.
                pack_data.extend_from_slice(&line);
            }
        }
    }

    Ok(pack_data)
}

// ---------------------------------------------------------------------------
// Pack file parsing
// ---------------------------------------------------------------------------

/// A parsed git object from a pack file.
struct PackObject {
    data: Vec<u8>,
    oid: gix_hash::ObjectId,
}

/// Parse a git pack file into individual objects.
fn parse_pack(data: &[u8]) -> anyhow::Result<Vec<PackObject>> {
    if data.len() < 12 {
        bail!("pack data too short");
    }
    if &data[..4] != b"PACK" {
        bail!("invalid pack signature");
    }
    let _version = u32::from_be_bytes(data[4..8].try_into()?);
    let num_objects = u32::from_be_bytes(data[8..12].try_into()?);

    let mut offset = 12;
    let mut objects = Vec::with_capacity(num_objects as usize);

    for _ in 0..num_objects {
        let (kind, size, header_end) = decode_pack_entry_header(data, offset)?;
        offset = header_end;

        // Decompress zlib data.
        let decompressed = decompress_zlib(&data[offset..], size)?;
        let consumed = find_zlib_end(&data[offset..], size)?;
        offset += consumed;

        // Compute the object's OID (git hashes the "<type> <size>\0<data>" form).
        let oid = compute_object_id(kind, &decompressed);
        objects.push(PackObject {
            data: decompressed,
            oid,
        });
    }

    Ok(objects)
}

/// Decode a pack entry header returning (kind, uncompressed_size, offset_after_header).
fn decode_pack_entry_header(
    data: &[u8],
    start: usize,
) -> anyhow::Result<(gix_object::Kind, usize, usize)> {
    let mut idx = start;
    if idx >= data.len() {
        bail!("unexpected end of pack data");
    }

    let byte = data[idx];
    let type_id = (byte >> 4) & 0x07;
    let mut size = (byte & 0x0f) as u64;
    let mut shift = 4u32;
    idx += 1;

    let mut prev = byte;
    while prev & 0x80 != 0 {
        if idx >= data.len() {
            bail!("unexpected end of pack data in header");
        }
        prev = data[idx];
        size |= ((prev & 0x7f) as u64) << shift;
        shift += 7;
        idx += 1;
    }

    let kind = match type_id {
        1 => gix_object::Kind::Commit,
        2 => gix_object::Kind::Tree,
        3 => gix_object::Kind::Blob,
        4 => gix_object::Kind::Tag,
        _ => {
            bail!("unsupported pack object type {type_id} (deltas not supported in shallow clone)")
        }
    };

    Ok((kind, size as usize, idx))
}

/// Decompress zlib data, expecting `expected_size` bytes of output.
fn decompress_zlib(compressed: &[u8], expected_size: usize) -> anyhow::Result<Vec<u8>> {
    use flate2::read::ZlibDecoder;
    use std::io::Read;

    let mut decoder = ZlibDecoder::new(compressed);
    let mut output = vec![0u8; expected_size];
    decoder
        .read_exact(&mut output)
        .context("failed to decompress pack object")?;
    Ok(output)
}

/// Find how many bytes the zlib stream consumed from the input.
fn find_zlib_end(compressed: &[u8], expected_size: usize) -> anyhow::Result<usize> {
    use flate2::read::ZlibDecoder;
    use std::io::Read;

    let mut decoder = ZlibDecoder::new(compressed);
    let mut buf = vec![0u8; expected_size];
    decoder.read_exact(&mut buf)?;
    Ok(decoder.total_in() as usize)
}

/// Compute a git object ID: SHA-1 of `"<type> <size>\0<data>"`.
fn compute_object_id(kind: gix_object::Kind, data: &[u8]) -> gix_hash::ObjectId {
    ids::ObjId::from_git_object(kind, data).into()
}

// ---------------------------------------------------------------------------
// Tree extraction
// ---------------------------------------------------------------------------

/// Extract a commit's tree to the filesystem.
fn extract_tree(objects: &[PackObject], commit_hash: &str, dest_dir: &Path) -> anyhow::Result<()> {
    // Build an OID -> object index.
    let mut by_oid: HashMap<gix_hash::ObjectId, &PackObject> = HashMap::new();
    for obj in objects {
        by_oid.insert(obj.oid, obj);
    }

    // Find the commit. ObjId validates first; convert to gix's `ObjectId`
    // only at the lookup boundary.
    let commit_id: ids::ObjId = commit_hash
        .parse()
        .map_err(|e| anyhow!("invalid commit hash: {e}"))?;
    let commit_oid: gix_hash::ObjectId = commit_id.into();
    let commit_obj = by_oid
        .get(&commit_oid)
        .ok_or_else(|| anyhow!("commit {commit_hash} not found in pack"))?;

    // Extract the tree OID from the commit.
    let tree_oid = gix_object::CommitRefIter::from_bytes(&commit_obj.data)
        .tree_id()
        .context("failed to parse commit tree")?;

    // Recursively extract the tree.
    extract_tree_recursive(&by_oid, tree_oid, dest_dir)?;
    Ok(())
}

fn extract_tree_recursive(
    objects: &HashMap<gix_hash::ObjectId, &PackObject>,
    tree_oid: gix_hash::ObjectId,
    dest_dir: &Path,
) -> anyhow::Result<()> {
    let tree_obj = objects
        .get(&tree_oid)
        .ok_or_else(|| anyhow!("tree {} not found in pack", tree_oid))?;

    std::fs::create_dir_all(dest_dir)?;

    for entry in gix_object::TreeRefIter::from_bytes(&tree_obj.data) {
        let entry = entry?;
        let name = std::str::from_utf8(entry.filename)?;
        let child_oid = entry.oid.to_owned();
        let child_path = dest_dir.join(name);

        if entry.mode.is_tree() {
            extract_tree_recursive(objects, child_oid, &child_path)?;
        } else if (entry.mode.is_blob() || entry.mode.is_executable())
            && let Some(blob) = objects.get(&child_oid)
        {
            std::fs::write(&child_path, &blob.data)?;
        }
        // Skip symlinks and submodules.
    }
    Ok(())
}

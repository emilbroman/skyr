//! BuildKit integration for building and pushing container images.
//!
//! This module provides the interface for building container images using BuildKit
//! and pushing them to a container registry.
//!
//! Implementation note: We use the `buildctl` CLI subprocess rather than native gRPC
//! because the available Rust BuildKit clients have issues:
//! - `buildkit-client` v0.1.4 has proto definition mismatches (fails to compile)
//! - `buildkit-rs` is archived and unmaintained
//!
//! When a stable Rust BuildKit client becomes available, this module should be
//! updated to use native gRPC for better performance and error handling.

use std::path::Path;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::info;

use crate::PluginError;

/// Result of a successful image build.
#[derive(Debug, Clone)]
pub struct BuildResult {
    /// Full image reference including registry and digest (e.g., "registry:5000/name@sha256:...")
    pub fullname: String,
    /// Image digest (e.g., "sha256:...")
    pub digest: String,
}

/// Build a container image and push it to the registry.
///
/// # Arguments
///
/// * `buildkit_addr` - BuildKit server address (e.g., "tcp://buildkit:1234")
/// * `context_path` - Local path to the build context directory
/// * `containerfile` - Path to the Containerfile/Dockerfile relative to context
/// * `image_name` - Image name (without registry prefix)
/// * `registry_url` - Container registry URL (e.g., "http://registry:5000")
///
/// # Returns
///
/// A `BuildResult` containing the full image reference and digest.
pub async fn build_and_push(
    buildkit_addr: &str,
    context_path: &Path,
    containerfile: &str,
    image_name: &str,
    registry_url: &str,
    log: &ldb::NamespacePublisher,
) -> Result<BuildResult, PluginError> {
    // Parse the registry URL to extract the host:port
    let registry_host = parse_registry_host(registry_url)?;

    // Build the full image reference (we'll add the digest later)
    let image_ref = format!("{}/{}", registry_host, image_name);

    info!(
        image_ref = %image_ref,
        context_path = %context_path.display(),
        containerfile = %containerfile,
        "starting buildctl build"
    );

    // Build the buildctl command
    // buildctl build \
    //   --addr <buildkit_addr> \
    //   --frontend dockerfile.v0 \
    //   --local context=<context_path> \
    //   --local dockerfile=<context_path> \
    //   --opt filename=<containerfile> \
    //   --output type=image,name=<image_ref>,push=true \
    //   --metadata-file /dev/stdout

    let mut child = Command::new("/usr/bin/buildctl")
        .arg("--addr")
        .arg(buildkit_addr)
        .arg("build")
        .arg("--frontend")
        .arg("dockerfile.v0")
        .arg("--local")
        .arg(format!("context={}", context_path.display()))
        .arg("--local")
        .arg(format!("dockerfile={}", context_path.display()))
        .arg("--opt")
        .arg(format!("filename={}", containerfile))
        .arg("--output")
        .arg(format!(
            "type=image,name={},push=true,registry.insecure=true",
            image_ref
        ))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| PluginError::ImageBuild(format!("failed to run buildctl: {e}")))?;

    // Stream stdout and stderr lines to LDB as they arrive.
    // We only retain the digest (if found) and the last stderr line for error reporting,
    // rather than accumulating all output.
    let stdout_pipe = child.stdout.take().expect("stdout was piped");
    let stderr_pipe = child.stderr.take().expect("stderr was piped");

    let stdout_log = log.clone();
    let stderr_log = log.clone();

    let stdout_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stdout_pipe).lines();
        let mut digest = None;
        while let Ok(Some(line)) = lines.next_line().await {
            if digest.is_none() {
                digest = try_extract_digest(&line);
            }
            stdout_log.info(line).await;
        }
        digest
    });

    let stderr_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr_pipe).lines();
        let mut digest = None;
        let mut last_line = String::new();
        while let Ok(Some(line)) = lines.next_line().await {
            if digest.is_none() {
                digest = try_extract_digest(&line);
            }
            stderr_log.error(line.clone()).await;
            last_line = line;
        }
        (digest, last_line)
    });

    let status = child
        .wait()
        .await
        .map_err(|e| PluginError::ImageBuild(format!("failed to wait for buildctl: {e}")))?;

    let stdout_digest = stdout_task.await.unwrap_or_default();
    let (stderr_digest, last_stderr) = stderr_task.await.unwrap_or_default();

    if !status.success() {
        return Err(PluginError::ImageBuild(format!(
            "buildctl failed with status {}: {}",
            status, last_stderr
        )));
    }

    let digest = stdout_digest.or(stderr_digest).ok_or_else(|| {
        PluginError::ImageBuild(
            "could not extract image digest from buildctl output".to_string(),
        )
    })?;

    // Build the full image reference with digest
    let fullname = format!("{}@{}", image_ref, digest);

    info!(
        fullname = %fullname,
        digest = %digest,
        "image built and pushed successfully"
    );

    Ok(BuildResult { fullname, digest })
}

/// Parse a registry URL to extract the host:port.
fn parse_registry_host(registry_url: &str) -> Result<String, PluginError> {
    // Handle URLs like "http://registry:5000" or "https://registry:5000"
    // or just "registry:5000"
    let url = registry_url
        .trim_start_matches("http://")
        .trim_start_matches("https://");

    // Remove any trailing path
    let host = url.split('/').next().unwrap_or(url);

    if host.is_empty() {
        return Err(PluginError::InvalidInput(format!(
            "invalid registry URL: {registry_url}"
        )));
    }

    Ok(host.to_string())
}

/// Try to extract a sha256 image digest from a single line of output.
fn try_extract_digest(line: &str) -> Option<String> {
    let pos = line.find("sha256:")?;
    let rest = &line[pos..];
    if rest.len() < 7 {
        return None;
    }
    let hex_part = &rest[7..];
    let hex_len = hex_part
        .chars()
        .take_while(|c| c.is_ascii_hexdigit())
        .count();
    if hex_len >= 64 {
        // "sha256:" (7) + 64 hex chars = 71
        Some(rest[..7 + hex_len.min(64)].to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_registry_host() {
        assert_eq!(
            parse_registry_host("http://registry:5000").unwrap(),
            "registry:5000"
        );
        assert_eq!(
            parse_registry_host("https://registry:5000").unwrap(),
            "registry:5000"
        );
        assert_eq!(
            parse_registry_host("registry:5000").unwrap(),
            "registry:5000"
        );
        assert_eq!(
            parse_registry_host("http://localhost:5000/").unwrap(),
            "localhost:5000"
        );
    }

    #[test]
    fn test_try_extract_digest() {
        // sha256: (7) + 64 hex chars = 71 total
        let line = "exporting to image sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789 done";
        let digest = try_extract_digest(line).unwrap();
        assert!(digest.starts_with("sha256:"));
        assert_eq!(digest.len(), 71);

        let line2 = "pushing manifest sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let digest2 = try_extract_digest(line2).unwrap();
        assert!(digest2.starts_with("sha256:"));
        assert_eq!(digest2.len(), 71);

        assert!(try_extract_digest("no digest here").is_none());
        assert!(try_extract_digest("sha256:tooshort").is_none());
    }
}

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

use tokio::process::Command;
use tracing::{debug, info};

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

    let output = Command::new("/usr/bin/buildctl")
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
        .output()
        .await
        .map_err(|e| PluginError::ImageBuild(format!("failed to run buildctl: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(PluginError::ImageBuild(format!(
            "buildctl failed with status {}: {}",
            output.status, stderr
        )));
    }

    // Parse the output to get the digest
    // buildctl outputs something like: "exporting to image" and the digest is in the output
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    debug!(stdout = %stdout, stderr = %stderr, "buildctl output");

    // Extract the digest from the output
    // The format varies, but typically includes "sha256:..." in the output
    let digest = extract_digest(&stdout, &stderr)?;

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

/// Extract the image digest from buildctl output.
fn extract_digest(stdout: &str, stderr: &str) -> Result<String, PluginError> {
    // BuildKit outputs the digest in various formats depending on the version
    // Common patterns:
    // - "sha256:abc123..." somewhere in the output
    // - "digest: sha256:abc123..."
    // - In metadata JSON output

    // Search in both stdout and stderr
    let combined = format!("{}\n{}", stdout, stderr);

    // Look for sha256: pattern
    for line in combined.lines() {
        if let Some(pos) = line.find("sha256:") {
            // Extract the digest (sha256: followed by 64 hex chars)
            let rest = &line[pos..];

            // Skip the "sha256:" prefix (7 chars) and count hex digits
            if rest.len() >= 7 {
                let hex_part = &rest[7..];
                let hex_len = hex_part
                    .chars()
                    .take_while(|c| c.is_ascii_hexdigit())
                    .count();

                if hex_len >= 64 {
                    // "sha256:" (7) + 64 hex chars = 71
                    let digest = &rest[..7 + hex_len.min(64)];
                    return Ok(digest.to_string());
                }
            }
        }
    }

    // If we can't find the digest, return an error
    Err(PluginError::ImageBuild(
        "could not extract image digest from buildctl output".to_string(),
    ))
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
    fn test_extract_digest() {
        // sha256: (7) + 64 hex chars = 71 total
        let output = "exporting to image sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789 done";
        let digest = extract_digest(output, "").unwrap();
        assert!(digest.starts_with("sha256:"));
        assert_eq!(digest.len(), 71);

        let output2 = "pushing manifest sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let digest2 = extract_digest(output2, "").unwrap();
        assert!(digest2.starts_with("sha256:"));
        assert_eq!(digest2.len(), 71);
    }
}

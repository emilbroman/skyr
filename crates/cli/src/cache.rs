//! Local disk cache for non-local packages.
//!
//! Packages are cached under `~/.cache/skyr-packages/` with the layout:
//!
//! ```text
//! ~/.cache/skyr-packages/
//! ├── Std/0.1.0/              # stdlib extracted from the binary
//! │   ├── Time.scl
//! │   └── ...
//! ├── MyOrg-MyRepo/<commit>/  # git packages keyed by commit hash
//! │   ├── Main.scl
//! │   └── ...
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;

use crate::auth;

/// Returns the cache root directory (`~/.cache/skyr-packages/`).
pub fn cache_root() -> anyhow::Result<PathBuf> {
    Ok(auth::home_dir()?.join(".cache").join("skyr-packages"))
}

/// Converts a [`sclc::PackageId`] display string to a filesystem-safe
/// directory name by replacing `/` with `-`.
pub fn package_dir_name(pkg_id: &sclc::PackageId) -> String {
    pkg_id.to_string().replace('/', "-")
}

/// Returns the full path for a cached package at a specific version.
pub fn package_version_dir(pkg_id: &sclc::PackageId, version: &str) -> anyhow::Result<PathBuf> {
    Ok(cache_root()?.join(package_dir_name(pkg_id)).join(version))
}

/// Returns `true` if the cache directory for this package+version exists.
pub fn is_cached(pkg_id: &sclc::PackageId, version: &str) -> anyhow::Result<bool> {
    let dir = package_version_dir(pkg_id, version)?;
    Ok(dir.is_dir())
}

/// Atomically moves `tmp` to `target` with race-condition handling.
///
/// If the rename fails because another process already placed the directory,
/// the temporary directory is cleaned up. If the parent directory is missing,
/// it is created and the rename is retried once.
pub(crate) async fn atomic_rename_dir(tmp: &Path, target: &Path) -> anyhow::Result<()> {
    if let Err(_e) = tokio::fs::rename(tmp, target).await {
        if target.is_dir() {
            let _ = tokio::fs::remove_dir_all(tmp).await;
            return Ok(());
        }
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::rename(tmp, target).await.with_context(|| {
            format!("failed to rename {} to {}", tmp.display(), target.display())
        })?;
    }
    Ok(())
}

/// Writes the bundled stdlib `.scl` files to disk under the cache directory.
///
/// Returns the path to the populated stdlib cache directory.
/// If the directory already exists, this is a no-op.
pub async fn populate_stdlib() -> anyhow::Result<PathBuf> {
    let pkg_id = sclc::PackageId::from(["Std"]);
    let version = sclc::VERSION;
    let dir = package_version_dir(&pkg_id, version)?;

    if dir.is_dir() {
        return Ok(dir);
    }

    // Write to a temp directory first, then atomically rename.
    let tmp = dir.with_extension("tmp");
    if tmp.exists() {
        tokio::fs::remove_dir_all(&tmp).await?;
    }
    tokio::fs::create_dir_all(&tmp)
        .await
        .with_context(|| format!("failed to create {}", tmp.display()))?;

    for (filename, content) in sclc::bundled_stdlib_files() {
        let dest = tmp.join(filename);
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&dest, content).await?;
    }

    atomic_rename_dir(&tmp, &dir).await?;

    Ok(dir)
}

/// Builds a [`sclc::CompositePackageFinder`] that includes the user's local
/// package, all resolved cached dependencies (as [`sclc::FsPackage`]
/// instances), and the standard library.
pub fn build_cached_finder(
    user_package: Arc<dyn sclc::Package>,
    resolved: &[ResolvedPackage],
) -> Arc<sclc::CompositePackageFinder> {
    let std_pkg = Arc::new(sclc::StdPackage::new());

    let mut finders: Vec<Arc<dyn sclc::PackageFinder>> = Vec::new();
    finders.push(sclc::wrap_as_finder(user_package));

    for rp in resolved {
        let fs_pkg = sclc::FsPackage::new(rp.cache_dir.clone(), rp.package_id.clone());
        finders.push(sclc::wrap_as_finder(Arc::new(fs_pkg)));
    }

    // StdPackage goes last (provides register_externs).
    finders.push(sclc::wrap_as_finder(std_pkg));

    Arc::new(sclc::CompositePackageFinder::new(finders))
}

/// A resolved and cached dependency.
#[derive(Clone, Debug)]
pub struct ResolvedPackage {
    pub package_id: sclc::PackageId,
    pub cache_dir: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_dir_name_single_segment() {
        let id = sclc::PackageId::from(["Std"]);
        assert_eq!(package_dir_name(&id), "Std");
    }

    #[test]
    fn package_dir_name_two_segments() {
        let id = sclc::PackageId::from(["MyOrg", "MyRepo"]);
        assert_eq!(package_dir_name(&id), "MyOrg-MyRepo");
    }

    #[test]
    fn cache_root_under_home() {
        // Just verify it doesn't panic and ends with the expected suffix.
        if let Ok(root) = cache_root() {
            assert!(root.ends_with(".cache/skyr-packages"));
        }
    }

    #[test]
    fn version_dir_structure() {
        let id = sclc::PackageId::from(["MyOrg", "MyRepo"]);
        if let Ok(dir) = package_version_dir(&id, "abc123") {
            assert!(dir.ends_with("MyOrg-MyRepo/abc123"));
        }
    }
}

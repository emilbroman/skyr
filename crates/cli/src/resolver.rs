//! Recursive dependency resolution from `Package.scle` manifests.
//!
//! Starting from a root package, reads its manifest, resolves each
//! dependency specifier to a concrete commit hash, populates the local
//! cache, and then recursively processes transitive dependencies.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, bail};
use ids::RepoQid;

use crate::cache::{self, ResolvedPackage};
use crate::git_client::GitClient;

/// Parse a slash-separated package string (e.g. `"MyOrg/MyRepo"`) into a
/// [`sclc::PackageId`], filtering out empty segments.
pub fn parse_package_id(package: &str) -> sclc::PackageId {
    package
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect::<sclc::PackageId>()
}

/// Load the manifest for `user_package`, and if it declares dependencies,
/// create a [`GitClient`] and resolve them all (direct + transitive).
///
/// Returns an empty vec when there is no manifest or no dependencies,
/// avoiding the SSH handshake required to construct a [`GitClient`].
pub async fn resolve_package_deps(
    user_package: Arc<dyn sclc::Package>,
    default_finder: Arc<dyn sclc::PackageFinder>,
    git_server: &str,
) -> anyhow::Result<Vec<ResolvedPackage>> {
    let manifest = sclc::load_manifest(Arc::clone(&user_package), default_finder.clone()).await?;
    let Some(manifest) = manifest else {
        return Ok(Vec::new());
    };
    if manifest.dependencies.is_empty() {
        return Ok(Vec::new());
    }

    let git_client = GitClient::from_config(git_server.to_string()).await?;
    resolve_all(user_package, default_finder, &git_client).await
}

/// Resolve all dependencies of `root_package` (direct + transitive),
/// populating the local cache as needed.
///
/// Returns the list of all resolved packages, each backed by a cache
/// directory that can be wrapped in an [`sclc::FsPackage`].
pub async fn resolve_all(
    root_package: Arc<dyn sclc::Package>,
    root_finder: Arc<dyn sclc::PackageFinder>,
    git_client: &GitClient,
) -> anyhow::Result<Vec<ResolvedPackage>> {
    let mut resolved: Vec<ResolvedPackage> = Vec::new();
    let mut visited: HashSet<(RepoQid, String)> = HashSet::new();
    let mut in_progress: HashSet<RepoQid> = HashSet::new();

    // Always populate stdlib in the cache (needed for transitive manifest
    // evaluation — each cached package's Package.scle imports Std/Package).
    cache::populate_stdlib().await?;

    let manifest = sclc::load_manifest(root_package, root_finder)
        .await
        .context("failed to load root Package.scle")?;

    let Some(manifest) = manifest else {
        return Ok(resolved);
    };

    resolve_deps(
        &manifest.dependencies,
        git_client,
        &mut resolved,
        &mut visited,
        &mut in_progress,
    )
    .await?;

    Ok(resolved)
}

/// Recursively resolve a set of dependencies.
fn resolve_deps<'a>(
    dependencies: &'a BTreeMap<RepoQid, sclc::Specifier>,
    git_client: &'a GitClient,
    resolved: &'a mut Vec<ResolvedPackage>,
    visited: &'a mut HashSet<(RepoQid, String)>,
    in_progress: &'a mut HashSet<RepoQid>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
    Box::pin(async move {
        for (repo_qid, specifier) in dependencies {
            // Resolve specifier to a commit hash.
            let commit_hash = git_client
                .resolve_ref(repo_qid, specifier)
                .await
                .with_context(|| {
                    format!("failed to resolve {} = {}", repo_qid, specifier.to_raw())
                })?;

            // Skip if already resolved at this exact version.
            let key = (repo_qid.clone(), commit_hash.clone());
            if visited.contains(&key) {
                continue;
            }

            // Check for version conflicts (same repo, different hash).
            if let Some(existing) = resolved.iter().find(|r| {
                r.package_id.as_slice() == [repo_qid.org.to_string(), repo_qid.repo.to_string()]
            }) {
                let existing_version = existing
                    .cache_dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");
                if existing_version != commit_hash {
                    bail!(
                        "conflicting versions for {}: already resolved to {}, \
                         but also required at {}",
                        repo_qid,
                        existing_version,
                        commit_hash,
                    );
                }
                continue;
            }

            // Detect cycles.
            if !in_progress.insert(repo_qid.clone()) {
                bail!("dependency cycle detected involving {}", repo_qid);
            }

            // Cache the package.
            let package_id =
                sclc::PackageId::from([repo_qid.org.to_string(), repo_qid.repo.to_string()]);

            let cache_dir = if !cache::is_cached(&package_id, &commit_hash)? {
                let dir = cache::package_version_dir(&package_id, &commit_hash)?;
                let tmp = dir.with_extension("tmp");
                if tmp.exists() {
                    tokio::fs::remove_dir_all(&tmp).await?;
                }
                tokio::fs::create_dir_all(&tmp).await?;

                git_client
                    .fetch_tree_at_commit(repo_qid, &commit_hash, &tmp)
                    .await
                    .with_context(|| format!("failed to fetch {} at {}", repo_qid, commit_hash))?;

                cache::atomic_rename_dir(&tmp, &dir).await?;
                dir
            } else {
                cache::package_version_dir(&package_id, &commit_hash)?
            };

            visited.insert(key);
            resolved.push(ResolvedPackage {
                package_id: package_id.clone(),
                cache_dir: cache_dir.clone(),
            });

            // Load transitive dependencies from the cached package's manifest.
            let transitive_deps = load_cached_manifest(&package_id, &cache_dir).await?;

            if let Some(manifest) = transitive_deps {
                resolve_deps(
                    &manifest.dependencies,
                    git_client,
                    resolved,
                    visited,
                    in_progress,
                )
                .await?;
            }

            in_progress.remove(repo_qid);
        }

        Ok(())
    })
}

/// Load a manifest from a cached package directory.
async fn load_cached_manifest(
    package_id: &sclc::PackageId,
    cache_dir: &Path,
) -> anyhow::Result<Option<sclc::Manifest>> {
    let fs_pkg: Arc<dyn sclc::Package> = Arc::new(sclc::FsPackage::new(
        cache_dir.to_path_buf(),
        package_id.clone(),
    ));
    let std_pkg: Arc<dyn sclc::Package> = Arc::new(sclc::StdPackage::new());

    let finder: Arc<dyn sclc::PackageFinder> = Arc::new(sclc::CompositePackageFinder::new(vec![
        sclc::wrap_as_finder(Arc::clone(&fs_pkg)),
        sclc::wrap_as_finder(std_pkg),
    ]));

    sclc::load_manifest(fs_pkg, finder).await.with_context(|| {
        format!(
            "failed to load transitive manifest for {} at {}",
            package_id,
            cache_dir.display()
        )
    })
}

//! Cross-repository [`PackageFinder`] backed by a manifest and the CDB.
//!
//! Each deployment that imports modules from foreign repositories is
//! configured with a manifest mapping `Org/Repo → Specifier`. The
//! [`CrossRepoPackageFinder`] consults this map when resolving a raw module
//! id whose first two segments name a foreign repo, looks up the resolved
//! deployment via the CDB, and returns a [`Package`] backed by a
//! [`cdb::DeploymentClient`].
//!
//! ## v1 limitations
//!
//! - Cross-organization imports are rejected (the importer's `local_org`
//!   must match the dependency's org).
//! - Diamond dependencies that resolve to *different* revisions of the same
//!   foreign repo are not supported. The finder treats each foreign repo as
//!   having a single resolved revision per deployment. The
//!   `CROSS_REPO_IMPORTS.md` design document describes a future
//!   "@hash-in-package-id" rewrite that lifts this restriction.
//! - Access control (UDB org-membership) is the caller's responsibility —
//!   the finder is given the manifest after the caller has authorised it.

#![cfg(feature = "cdb")]

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use futures_util::StreamExt;
use ids::{DeploymentQid, OrgId, RepoQid};
use thiserror::Error;
use tokio::sync::RwLock;

use crate::{CachedPackage, LoadError, Package, PackageFinder, PackageId, Specifier};

/// A cross-repo [`PackageFinder`].
///
/// See the module-level documentation for design notes and v1 limitations.
pub struct CrossRepoPackageFinder {
    cdb: cdb::Client,
    local_org: OrgId,
    dependencies: BTreeMap<RepoQid, Specifier>,
    cache: RwLock<HashMap<RepoQid, Option<Arc<dyn Package>>>>,
    /// Resolved deployment QIDs alongside the cached packages, exposed for
    /// the DE so it can populate `EvalCtx::package_owner`.
    resolved: RwLock<HashMap<RepoQid, DeploymentQid>>,
}

/// Errors produced while resolving a foreign repo.
#[derive(Debug, Error)]
pub enum CrossRepoError {
    #[error("cross-org imports are not supported (importer is in {local}, dep is in {dep})")]
    CrossOrg { local: OrgId, dep: OrgId },

    #[error("dependency repo {0} does not exist")]
    NoSuchRepo(RepoQid),

    #[error(
        "dependency {repo} pinned to {specifier:?} could not be resolved to an active deployment"
    )]
    Unresolved { repo: RepoQid, specifier: Specifier },

    #[error("CDB error: {0}")]
    Cdb(Box<dyn std::error::Error + Send + Sync>),
}

impl CrossRepoPackageFinder {
    /// Construct a new finder.
    ///
    /// `local_org` is the organisation owning the importing repo. Foreign
    /// repos in any other org will be rejected (v1 has no cross-org imports).
    /// `dependencies` is the manifest's `dependencies` map.
    pub fn new(
        cdb: cdb::Client,
        local_org: OrgId,
        dependencies: BTreeMap<RepoQid, Specifier>,
    ) -> Self {
        Self {
            cdb,
            local_org,
            dependencies,
            cache: RwLock::new(HashMap::new()),
            resolved: RwLock::new(HashMap::new()),
        }
    }

    /// All currently-resolved foreign packages, keyed by their `Org/Repo`.
    /// Useful for `EvalCtx::set_package_owner` plumbing in the DE.
    pub async fn resolved_owners(&self) -> HashMap<RepoQid, DeploymentQid> {
        self.resolved.read().await.clone()
    }

    /// Whether any of the manifest's dependencies use a volatile (branch or
    /// tag) specifier. The DE uses this to gate the `Up` terminal-state
    /// transition (per `CROSS_REPO_IMPORTS.md` §6a — "Terminal-state rule").
    pub fn has_volatile_pins(&self) -> bool {
        self.dependencies.values().any(Specifier::is_volatile)
    }

    /// Direct lookup helper used by tests and (eventually) the DE: returns
    /// the resolved [`DeploymentQid`] for a foreign repo, resolving it
    /// through the CDB if not already cached. Returns `Ok(None)` when the
    /// repo isn't a declared dependency.
    pub async fn resolve_dependency(
        &self,
        repo: &RepoQid,
    ) -> Result<Option<DeploymentQid>, CrossRepoError> {
        if !self.dependencies.contains_key(repo) {
            return Ok(None);
        }
        // resolve_internal populates self.resolved.
        let _ = self.resolve_internal(repo).await?;
        Ok(self.resolved.read().await.get(repo).cloned())
    }

    /// Internal: resolve a foreign repo to a cached package, performing the
    /// CDB lookup once and caching the result.
    async fn resolve_internal(
        &self,
        repo: &RepoQid,
    ) -> Result<Option<Arc<dyn Package>>, CrossRepoError> {
        if let Some(cached) = self.cache.read().await.get(repo) {
            return Ok(cached.clone());
        }

        let Some(specifier) = self.dependencies.get(repo).cloned() else {
            return Ok(None);
        };

        if repo.org != self.local_org {
            return Err(CrossRepoError::CrossOrg {
                local: self.local_org.clone(),
                dep: repo.org.clone(),
            });
        }

        let deployment = self.lookup_deployment(repo, &specifier).await?;
        let dc = self.cdb.repo(repo.clone()).deployment(
            deployment.environment.environment.clone(),
            deployment.deployment.clone(),
            deployment.nonce,
        );
        let pkg: Arc<dyn Package> = Arc::new(CachedPackage::new(dc));

        self.cache
            .write()
            .await
            .insert(repo.clone(), Some(Arc::clone(&pkg)));
        self.resolved.write().await.insert(repo.clone(), deployment);
        Ok(Some(pkg))
    }

    async fn lookup_deployment(
        &self,
        repo: &RepoQid,
        specifier: &Specifier,
    ) -> Result<DeploymentQid, CrossRepoError> {
        let repo_client = self.cdb.repo(repo.clone());

        // Confirm the repo exists, surfacing a clearer error than the
        // empty-active-deployments case.
        match repo_client.get().await {
            Ok(_) => {}
            Err(cdb::RepositoryQueryError::NotFound) => {
                return Err(CrossRepoError::NoSuchRepo(repo.clone()));
            }
            Err(e) => return Err(CrossRepoError::Cdb(Box::new(e))),
        }

        match specifier {
            Specifier::Branch(name) => {
                let env: ids::EnvironmentId =
                    name.parse().map_err(|_| CrossRepoError::Unresolved {
                        repo: repo.clone(),
                        specifier: specifier.clone(),
                    })?;
                self.find_active_by_env(&repo_client, repo, specifier, env)
                    .await
            }
            Specifier::Tag(name) => {
                let env_str = format!("tag:{name}");
                let env: ids::EnvironmentId =
                    env_str.parse().map_err(|_| CrossRepoError::Unresolved {
                        repo: repo.clone(),
                        specifier: specifier.clone(),
                    })?;
                self.find_active_by_env(&repo_client, repo, specifier, env)
                    .await
            }
            Specifier::Hash(hex) => {
                let target_hash = hex.as_str();
                let mut deployments = repo_client
                    .deployments()
                    .await
                    .map_err(|e| CrossRepoError::Cdb(Box::new(e)))?;
                while let Some(result) = deployments.next().await {
                    let deployment = result.map_err(|e| CrossRepoError::Cdb(Box::new(e)))?;
                    if deployment.deployment.as_str() == target_hash {
                        return Ok(repo
                            .clone()
                            .environment(deployment.environment)
                            .deployment(deployment.deployment, deployment.nonce));
                    }
                }
                Err(CrossRepoError::Unresolved {
                    repo: repo.clone(),
                    specifier: specifier.clone(),
                })
            }
        }
    }

    async fn find_active_by_env(
        &self,
        repo_client: &cdb::RepositoryClient,
        repo: &RepoQid,
        specifier: &Specifier,
        env: ids::EnvironmentId,
    ) -> Result<DeploymentQid, CrossRepoError> {
        let mut deployments = repo_client
            .active_deployments()
            .await
            .map_err(|e| CrossRepoError::Cdb(Box::new(e)))?;
        while let Some(result) = deployments.next().await {
            let deployment = result.map_err(|e| CrossRepoError::Cdb(Box::new(e)))?;
            if deployment.environment == env {
                return Ok(repo
                    .clone()
                    .environment(deployment.environment)
                    .deployment(deployment.deployment, deployment.nonce));
            }
        }
        Err(CrossRepoError::Unresolved {
            repo: repo.clone(),
            specifier: specifier.clone(),
        })
    }
}

#[async_trait::async_trait]
impl PackageFinder for CrossRepoPackageFinder {
    async fn find(&self, raw_id: &[&str]) -> Result<Option<Arc<dyn Package>>, LoadError> {
        if raw_id.len() < 2 {
            return Ok(None);
        }
        let Ok(org) = raw_id[0].parse::<OrgId>() else {
            return Ok(None);
        };
        let Ok(repo_id) = raw_id[1].parse::<ids::RepoId>() else {
            return Ok(None);
        };
        let repo = RepoQid::new(org, repo_id);

        // Only respond for repos declared in our manifest. If the importer
        // refers to a non-declared repo, fall through to the next finder
        // (which will likely produce a "package not found" diagnostic).
        if !self.dependencies.contains_key(&repo) {
            return Ok(None);
        }

        match self.resolve_internal(&repo).await {
            Ok(Some(pkg)) => Ok(Some(pkg)),
            Ok(None) => Ok(None),
            Err(e) => {
                // Surface resolution errors as LoadError::Other so the
                // loader produces a diagnostic. We do not turn them into
                // "package not found" because that would mask the real
                // cause (e.g. cross-org rejection or unresolved branch).
                Err(LoadError::Other(Box::new(e)))
            }
        }
    }
}

/// Helper exposed so callers (the DE) can confirm a [`PackageId`] is
/// foreign — i.e. a two-segment id whose first two segments are in the
/// finder's dependency map.
pub fn package_id_for_repo(repo: &RepoQid) -> PackageId {
    [repo.org.to_string(), repo.repo.to_string()]
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deps_with(repo: &str, spec: &str) -> BTreeMap<RepoQid, Specifier> {
        let mut deps = BTreeMap::new();
        deps.insert(repo.parse().unwrap(), Specifier::parse(spec));
        deps
    }

    #[test]
    fn package_id_helper() {
        let repo: RepoQid = "MyOrg/MyRepo".parse().unwrap();
        let pid = package_id_for_repo(&repo);
        assert_eq!(pid.as_slice(), &["MyOrg".to_string(), "MyRepo".to_string()]);
    }

    #[test]
    fn volatility_check() {
        // We can't construct a finder without a real CDB client, but we can
        // test the property in isolation by directly building the field.
        let deps = deps_with("MyOrg/Repo", "main");
        assert!(deps.values().any(Specifier::is_volatile));

        let pinned = deps_with("MyOrg/Repo", "b50d18287a6a3b86c3f45e3a973a389784d353dd");
        assert!(!pinned.values().any(Specifier::is_volatile));
    }
}

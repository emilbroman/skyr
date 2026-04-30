//! Cross-repository [`PackageFinder`] backed by a manifest and the CDB.
//!
//! Each importer that depends on modules from foreign repositories is
//! configured with a manifest mapping `Org/Repo → Specifier`. The
//! [`CrossRepoPackageFinder`] consults this map when resolving a raw module
//! id whose first two segments name a foreign repo, resolves the specifier to
//! a commit hash via the CDB, and returns a [`Package`] backed by a
//! [`cdb::CommitClient`].
//!
//! Branch and tag specifiers resolve through the active deployment of the
//! foreign repo in the matching environment — that's how Skyr finds the
//! "current" commit for a moving ref. Hash specifiers bypass deployments
//! entirely and address a commit directly.
//!
//! ## v1 limitations
//!
//! - Cross-organization imports are rejected (the importer's `local_org`
//!   must match the dependency's org).
//! - Diamond dependencies that resolve to *different* revisions of the same
//!   foreign repo are not supported. The finder treats each foreign repo as
//!   having a single resolved revision per importer. The
//!   `CROSS_REPO_IMPORTS.md` design document describes a future
//!   "@hash-in-package-id" rewrite that lifts this restriction.
//! - Access control (UDB org-membership) is the caller's responsibility —
//!   the finder is given the manifest after the caller has authorised it.

#![cfg(feature = "cdb")]

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use futures_util::StreamExt;
use ids::{CommitHash, DeploymentQid, OrgId, RepoQid};
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
    /// Resolved commit hashes per dependency. Populated for every resolved
    /// dependency regardless of specifier kind. The DE uses this map as the
    /// compile cache key — recompilation is needed when, and only when, the
    /// resolved source for any dependency changes.
    resolved_commits: RwLock<HashMap<RepoQid, CommitHash>>,
    /// Deployment QIDs for dependencies that resolved through an active
    /// deployment (branch or tag specifiers). Hash-pinned dependencies do
    /// not appear here, so their effects fall back to the local owner —
    /// pinning by hash means "this exact code, on me", not "delegate to the
    /// foreign owner".
    resolved_owners: RwLock<HashMap<RepoQid, DeploymentQid>>,
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

    #[error("invalid commit hash in dependency specifier: {0}")]
    InvalidHash(String),

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
            resolved_commits: RwLock::new(HashMap::new()),
            resolved_owners: RwLock::new(HashMap::new()),
        }
    }

    /// All currently-resolved foreign packages that have a deployment owner,
    /// keyed by their `Org/Repo`. Hash-pinned dependencies are absent — they
    /// have no deployment owner and their effects fall back to the local
    /// owner. Used by the DE to populate `EvalCtx::set_package_owner`.
    pub async fn resolved_owners(&self) -> HashMap<RepoQid, DeploymentQid> {
        self.resolved_owners.read().await.clone()
    }

    /// Whether any of the manifest's dependencies use a volatile (branch or
    /// tag) specifier. The DE uses this to gate the `Up` terminal-state
    /// transition (per `CROSS_REPO_IMPORTS.md` §6a — "Terminal-state rule").
    pub fn has_volatile_pins(&self) -> bool {
        self.dependencies.values().any(Specifier::is_volatile)
    }

    /// Eagerly resolve every declared dependency to its current commit
    /// hash. Used by the DE to compute a cache key for the compiled ASG
    /// before deciding whether to recompile.
    pub async fn resolve_all(&self) -> Result<BTreeMap<RepoQid, CommitHash>, CrossRepoError> {
        let repos: Vec<RepoQid> = self.dependencies.keys().cloned().collect();
        let mut out = BTreeMap::new();
        for repo in repos {
            let _ = self.resolve_internal(&repo).await?;
            if let Some(commit) = self.resolved_commits.read().await.get(&repo).cloned() {
                out.insert(repo, commit);
            }
        }
        Ok(out)
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

        let resolution = self.lookup(repo, &specifier).await?;
        let cc = self
            .cdb
            .repo(repo.clone())
            .commit(resolution.commit.clone());
        let pkg: Arc<dyn Package> = Arc::new(CachedPackage::new(cc));

        self.cache
            .write()
            .await
            .insert(repo.clone(), Some(Arc::clone(&pkg)));
        self.resolved_commits
            .write()
            .await
            .insert(repo.clone(), resolution.commit);
        if let Some(owner) = resolution.owner {
            self.resolved_owners
                .write()
                .await
                .insert(repo.clone(), owner);
        }
        Ok(Some(pkg))
    }

    async fn lookup(
        &self,
        repo: &RepoQid,
        specifier: &Specifier,
    ) -> Result<Resolution, CrossRepoError> {
        match specifier {
            Specifier::Branch(name) => {
                let repo_client = self.cdb.repo(repo.clone());
                self.confirm_repo_exists(&repo_client, repo).await?;
                let env: ids::EnvironmentId =
                    name.parse().map_err(|_| CrossRepoError::Unresolved {
                        repo: repo.clone(),
                        specifier: specifier.clone(),
                    })?;
                self.resolve_via_active_deployment(&repo_client, repo, specifier, env)
                    .await
            }
            Specifier::Tag(name) => {
                let repo_client = self.cdb.repo(repo.clone());
                self.confirm_repo_exists(&repo_client, repo).await?;
                let env_str = format!("tag:{name}");
                let env: ids::EnvironmentId =
                    env_str.parse().map_err(|_| CrossRepoError::Unresolved {
                        repo: repo.clone(),
                        specifier: specifier.clone(),
                    })?;
                self.resolve_via_active_deployment(&repo_client, repo, specifier, env)
                    .await
            }
            Specifier::Hash(hex) => {
                // Hash specifiers bypass deployments. The pin addresses a
                // specific commit; whether anything is currently deployed at
                // that commit is irrelevant to package loading.
                let commit: CommitHash = hex
                    .parse()
                    .map_err(|_| CrossRepoError::InvalidHash(hex.clone()))?;
                Ok(Resolution {
                    commit,
                    owner: None,
                })
            }
        }
    }

    async fn confirm_repo_exists(
        &self,
        repo_client: &cdb::RepositoryClient,
        repo: &RepoQid,
    ) -> Result<(), CrossRepoError> {
        match repo_client.get().await {
            Ok(_) => Ok(()),
            Err(cdb::RepositoryQueryError::NotFound) => {
                Err(CrossRepoError::NoSuchRepo(repo.clone()))
            }
            Err(e) => Err(CrossRepoError::Cdb(Box::new(e))),
        }
    }

    async fn resolve_via_active_deployment(
        &self,
        repo_client: &cdb::RepositoryClient,
        repo: &RepoQid,
        specifier: &Specifier,
        env: ids::EnvironmentId,
    ) -> Result<Resolution, CrossRepoError> {
        let mut deployments = repo_client
            .active_deployments()
            .await
            .map_err(|e| CrossRepoError::Cdb(Box::new(e)))?;
        while let Some(result) = deployments.next().await {
            let deployment = result.map_err(|e| CrossRepoError::Cdb(Box::new(e)))?;
            if deployment.environment == env {
                let owner = repo
                    .clone()
                    .environment(deployment.environment)
                    .deployment(deployment.deployment.clone());
                return Ok(Resolution {
                    commit: deployment.deployment.commit,
                    owner: Some(owner),
                });
            }
        }
        Err(CrossRepoError::Unresolved {
            repo: repo.clone(),
            specifier: specifier.clone(),
        })
    }
}

/// Outcome of resolving a single dependency specifier.
struct Resolution {
    commit: CommitHash,
    /// `Some` for branch/tag specifiers (resolved through an active
    /// deployment); `None` for hash specifiers, which intentionally have no
    /// foreign owner.
    owner: Option<DeploymentQid>,
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

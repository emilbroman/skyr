use anyhow::anyhow;
use ids::RepoQid;

use crate::UserFacingError;
use crate::pools::IasPool;

/// Check that `username` is a member of `repo`'s organization.
///
/// Personal-org shortcut: if `repo.org == username` the caller is the
/// org's sole owner. Otherwise we resolve the org's home region in GDDB
/// and ask that region's IAS — org membership lives in UDB at the org's
/// home, which is not necessarily the same region as the repo or the
/// user.
pub(crate) async fn ensure_repo_access(
    username: &str,
    repo: &RepoQid,
    gddb_client: &gddb::Client,
    ias_pool: &IasPool,
) -> anyhow::Result<()> {
    if repo.org.as_str() == username {
        return Ok(());
    }

    let org_home = gddb_client
        .lookup_org(&repo.org)
        .await
        .map_err(|e| anyhow!("failed to look up org in GDDB: {e}"))?
        .ok_or_else(|| UserFacingError(format!("organization '{}' does not exist", repo.org)))?;

    let mut ias = ias_pool
        .for_region(&org_home)
        .await
        .map_err(|e| anyhow!("failed to connect to IAS in {org_home}: {e}"))?;

    let is_member = ias
        .org_contains_member(ias::proto::OrgContainsMemberRequest {
            name: repo.org.to_string(),
            username: username.to_string(),
        })
        .await
        .map_err(|status| anyhow!("IAS OrgContainsMember failed: {status}"))?
        .into_inner()
        .value;

    if !is_member {
        return Err(UserFacingError(format!(
            "permission denied: user '{username}' cannot access repository '{repo}'"
        ))
        .into());
    }

    Ok(())
}

pub(crate) async fn ensure_repo_exists(client: &cdb::Client, repo: &RepoQid) -> anyhow::Result<()> {
    match client.repo(repo.clone()).get().await {
        Ok(_) => Ok(()),
        Err(cdb::RepositoryQueryError::NotFound) => {
            Err(UserFacingError(format!("repository '{}' does not exist", repo)).into())
        }
        Err(err) => {
            tracing::error!("failed to query repository '{}': {}", repo, err);
            Err(UserFacingError("failed to access repository".to_string()).into())
        }
    }
}

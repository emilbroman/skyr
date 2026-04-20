use anyhow::anyhow;
use ids::RepoQid;

use crate::UserFacingError;

pub(crate) async fn ensure_repo_access(
    user: &udb::User,
    repo: &RepoQid,
    udb_client: &udb::Client,
) -> anyhow::Result<()> {
    // Personal org: username matches org name
    if repo.org.as_str() == user.username {
        return Ok(());
    }

    // Check org membership
    let is_member = udb_client
        .org(repo.org.as_str())
        .members()
        .contains(&user.username)
        .await
        .map_err(|e| anyhow!("failed to check org membership: {e}"))?;

    if !is_member {
        return Err(UserFacingError(format!(
            "permission denied: user '{}' cannot access repository '{}'",
            user.username, repo,
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

//! Recipient resolution for notification requests.
//!
//! A [`nq::NotificationRequest`] carries only the entity QID — never a
//! pre-snapshotted recipient list — because Skyr's IAM model is
//! "all members of the owning organization, always" and that membership can
//! change between RE enqueue and NE dispatch. We resolve recipients freshly at
//! send time so membership edits take effect immediately.
//!
//! The flow is:
//! 1. Parse the entity QID. It is either a deployment QID
//!    (`Org/Repo::Env@Dep.Nonce`) or a resource QID
//!    (`Org/Repo::Env::Type:Name`).
//! 2. Extract the owning [`ids::OrgId`] from the parsed QID.
//! 3. List the org's members from UDB.
//! 4. Look up each member's email address.
//!
//! Members whose user record is missing or whose email is empty are skipped
//! with a warning log; this keeps a partial-data UDB from blocking the entire
//! batch.

use ids::{DeploymentQid, OrgId, ResourceQid};
use thiserror::Error;
use udb::Client as UdbClient;

#[derive(Debug, Error)]
pub enum ResolveError {
    #[error("entity_qid is neither a valid deployment QID nor a valid resource QID: {0:?}")]
    InvalidEntityQid(String),

    #[error("organization {0:?} not found in UDB")]
    OrgNotFound(String),

    #[error("failed to query UDB: {0}")]
    Udb(String),
}

/// A resolved notification recipient.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipient {
    pub username: String,
    pub email: String,
}

/// Extracts the owning [`OrgId`] from an entity QID string.
///
/// Tries to parse the string first as a [`DeploymentQid`], falling back to
/// [`ResourceQid`]. Returns [`ResolveError::InvalidEntityQid`] if neither
/// parser accepts it.
pub fn extract_org_id(entity_qid: &str) -> Result<OrgId, ResolveError> {
    if let Ok(qid) = entity_qid.parse::<DeploymentQid>() {
        return Ok(qid.environment.repo.org);
    }
    if let Ok(qid) = entity_qid.parse::<ResourceQid>() {
        return Ok(qid.environment.repo.org);
    }
    Err(ResolveError::InvalidEntityQid(entity_qid.to_string()))
}

/// Resolves the full recipient list for a notification: every org member with
/// a non-empty email address.
///
/// This function does not return an error when individual members are missing
/// or have no email — those cases are logged and skipped. It only returns an
/// error if the org itself does not exist or a UDB call fails outright.
pub async fn resolve_recipients(
    udb: &UdbClient,
    entity_qid: &str,
) -> Result<Vec<Recipient>, ResolveError> {
    let org_id = extract_org_id(entity_qid)?;
    let org_name = org_id.to_string();

    // Confirm the org exists. `OrgClient::get` returns `OrgQueryError::NotFound`
    // when the org hash key is absent.
    let org_client = udb.org(&org_name);
    if let Err(err) = org_client.get().await {
        return match err {
            udb::OrgQueryError::NotFound => Err(ResolveError::OrgNotFound(org_name)),
            other => Err(ResolveError::Udb(other.to_string())),
        };
    }

    let members = org_client
        .members()
        .list()
        .await
        .map_err(|e| ResolveError::Udb(e.to_string()))?;

    let mut recipients = Vec::with_capacity(members.len());
    for username in members {
        match udb.user(&username).get().await {
            Ok(user) => {
                if user.email.is_empty() {
                    tracing::warn!(
                        username = %username,
                        "skipping notification recipient with empty email",
                    );
                    continue;
                }
                recipients.push(Recipient {
                    username: user.username,
                    email: user.email,
                });
            }
            Err(udb::UserQueryError::NotFound) => {
                tracing::warn!(
                    username = %username,
                    "skipping notification recipient: user record missing in UDB",
                );
            }
            Err(err) => {
                return Err(ResolveError::Udb(err.to_string()));
            }
        }
    }

    Ok(recipients)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_org_id_from_deployment_qid() {
        let qid = "MyOrg/MyRepo::main@2cbecbed4bfa1599ef4ce0dfc542c97a82d79268.a1b2c3d4e5f60718";
        let org = extract_org_id(qid).unwrap();
        assert_eq!(org.to_string(), "MyOrg");
    }

    #[test]
    fn extract_org_id_from_resource_qid() {
        let qid = "MyOrg/MyRepo::main::Std/Random.Number:my_number";
        let org = extract_org_id(qid).unwrap();
        assert_eq!(org.to_string(), "MyOrg");
    }

    #[test]
    fn extract_org_id_rejects_garbage() {
        assert!(matches!(
            extract_org_id("not-a-qid"),
            Err(ResolveError::InvalidEntityQid(_))
        ));
    }

    #[test]
    fn extract_org_id_rejects_partial_qid() {
        // Only the environment portion — neither a deployment nor a resource QID.
        assert!(matches!(
            extract_org_id("MyOrg/MyRepo::main"),
            Err(ResolveError::InvalidEntityQid(_))
        ));
    }
}

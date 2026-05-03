use std::sync::Arc;

mod graphql_ws;
mod json_scalar;
mod pools;
mod region_keys;
mod schema;
mod subscriptions;
mod webauthn;

use axum::{
    Json as AxumJson, Router,
    extract::{Extension, ws::WebSocketUpgrade},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use base64::Engine;
use clap::Parser;
use futures_util::StreamExt;
use http::StatusCode;
use juniper::{FieldResult, RootNode};
use sha2::Digest;
use tower_http::cors::{Any, CorsLayer};

use json_scalar::JsonValue;
use schema::{
    AuthChallenge, AuthSuccess, Deployment, Organization, Repository, SignedInUser, UserData,
};
use webauthn::{ProofKind, proof_from_json};

#[derive(Clone, Debug)]
pub(crate) struct AuthenticatedUser {
    pub(crate) username: String,
    pub(crate) home_region: ids::RegionId,
}

#[derive(Clone)]
pub(crate) struct Context {
    pub(crate) ias_pool: pools::IasPool,
    pub(crate) cdb_pool: pools::CdbPool,
    pub(crate) sdb_pool: pools::SdbPool,
    pub(crate) ldb_consumer_pool: pools::LdbConsumerPool,
    pub(crate) ldb_publisher_pool: pools::LdbPublisherPool,
    pub(crate) gddb_client: gddb::Client,
    pub(crate) rdb_pool: pools::RdbPool,
    pub(crate) adb_client: adb::Client,
    pub(crate) rtq_publisher: rtq::Publisher,
    pub(crate) rp_id: Arc<String>,
    pub(crate) rp_name: Arc<String>,
    pub(crate) authenticated_user: Option<AuthenticatedUser>,
}

impl Context {
    pub(crate) async fn check_auth(&self) -> FieldResult<AuthenticatedUser> {
        self.authenticated_user
            .clone()
            .ok_or_else(|| field_error("Not authenticated"))
    }

    /// Resolve `org`'s home region via GDDB.
    pub(crate) async fn home_region_for_org(&self, org: &ids::OrgId) -> FieldResult<ids::RegionId> {
        let home = self.gddb_client.lookup_org(org).await.map_err(|e| {
            tracing::error!("Failed to look up org in GDDB: {}", e);
            internal_error()
        })?;
        home.ok_or_else(|| field_error("Organization not found"))
    }

    /// Resolve `qid`'s home region via GDDB.
    pub(crate) async fn home_region_for_repo(
        &self,
        qid: &ids::RepoQid,
    ) -> FieldResult<ids::RegionId> {
        let home = self.gddb_client.lookup_repo(qid).await.map_err(|e| {
            tracing::error!("Failed to look up repo in GDDB: {}", e);
            internal_error()
        })?;
        home.ok_or_else(|| field_error("Repository not found"))
    }

    /// Resolve a user's home region. Users are personal orgs of the same
    /// name, so this is just `home_region_for_org` with the username
    /// parsed as an `OrgId`.
    pub(crate) async fn home_region_for_user(&self, username: &str) -> FieldResult<ids::RegionId> {
        let org_id: ids::OrgId = username
            .parse()
            .map_err(|_| field_error("Invalid username"))?;
        self.home_region_for_org(&org_id).await
    }

    pub(crate) async fn ias_for_region(
        &self,
        region: &ids::RegionId,
    ) -> FieldResult<ias::IdentityAndAccessClient> {
        self.ias_pool.for_region(region).await.map_err(|e| {
            tracing::error!("Failed to connect to IAS in {}: {}", region, e);
            internal_error()
        })
    }

    pub(crate) async fn cdb_for_region(&self, region: &ids::RegionId) -> FieldResult<cdb::Client> {
        self.cdb_pool.for_region(region).await.map_err(|e| {
            tracing::error!("Failed to connect to CDB in {}: {}", region, e);
            internal_error()
        })
    }

    pub(crate) async fn rdb_for_region(&self, region: &ids::RegionId) -> FieldResult<rdb::Client> {
        self.rdb_pool.for_region(region).await.map_err(|e| {
            tracing::error!("Failed to connect to RDB in {}: {}", region, e);
            internal_error()
        })
    }

    pub(crate) async fn sdb_for_region(&self, region: &ids::RegionId) -> FieldResult<sdb::Client> {
        self.sdb_pool.for_region(region).await.map_err(|e| {
            tracing::error!("Failed to connect to SDB in {}: {}", region, e);
            internal_error()
        })
    }

    pub(crate) async fn ldb_consumer_for_region(
        &self,
        region: &ids::RegionId,
    ) -> FieldResult<ldb::Consumer> {
        self.ldb_consumer_pool
            .for_region(region)
            .await
            .map_err(|e| {
                tracing::error!("Failed to connect to LDB consumer in {}: {}", region, e);
                internal_error()
            })
    }

    pub(crate) async fn ldb_publisher_for_region(
        &self,
        region: &ids::RegionId,
    ) -> FieldResult<ldb::Publisher> {
        self.ldb_publisher_pool
            .for_region(region)
            .await
            .map_err(|e| {
                tracing::error!("Failed to connect to LDB publisher in {}: {}", region, e);
                internal_error()
            })
    }
}

impl juniper::Context for Context {}

pub(crate) fn field_error(message: &str) -> juniper::FieldError {
    juniper::FieldError::new(message, juniper::Value::Null)
}

pub(crate) fn internal_error() -> juniper::FieldError {
    field_error("Internal server error")
}

/// Convert a `tonic::Status` from an IAS RPC into a GraphQL field error.
/// User-facing IAS error codes (Unauthenticated, NotFound, AlreadyExists,
/// InvalidArgument, FailedPrecondition) pass their message through;
/// anything else logs and surfaces as a generic internal error.
pub(crate) fn map_ias_status(status: tonic::Status) -> juniper::FieldError {
    match status.code() {
        tonic::Code::Unauthenticated
        | tonic::Code::NotFound
        | tonic::Code::AlreadyExists
        | tonic::Code::InvalidArgument
        | tonic::Code::FailedPrecondition => field_error(status.message()),
        _ => {
            tracing::error!("IAS RPC failed: {status}");
            internal_error()
        }
    }
}

fn parse_region_arg(requested: &str) -> FieldResult<ids::RegionId> {
    requested.parse().map_err(|_| field_error("Invalid region"))
}

/// Basic email validation: requires exactly one `@`, non-empty local and domain
/// parts, a dot in the domain, and no whitespace.
fn is_valid_email(email: &str) -> bool {
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    if local.is_empty() || domain.is_empty() {
        return false;
    }
    if email.contains(char::is_whitespace) {
        return false;
    }
    let Some((domain_name, tld)) = domain.rsplit_once('.') else {
        return false;
    };
    if domain_name.is_empty() || tld.is_empty() {
        return false;
    }
    if domain.contains('@') {
        return false;
    }
    true
}

struct Query;

static USERNAME_REGEX: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"^[a-zA-Z0-9_-]{3,20}$").unwrap());

#[juniper::graphql_object(Context = Context)]
impl Query {
    async fn health(context: &Context) -> bool {
        let _ = (&context.cdb_pool, &context.rdb_pool, &context.adb_client);
        tokio::task::yield_now().await;
        true
    }

    async fn me(context: &Context) -> FieldResult<SignedInUser> {
        let auth = context.check_auth().await?;
        let mut ias = context.ias_for_region(&auth.home_region).await?;
        let user = ias
            .get_user(ias::proto::GetUserRequest {
                username: auth.username.clone(),
            })
            .await
            .map_err(map_ias_status)?
            .into_inner();
        Ok(SignedInUser {
            user: UserData::from(user),
        })
    }

    /// Issue an authentication challenge for `username`.
    ///
    /// The challenge is owned by the user's home-region IAS (which holds
    /// the salt). This edge resolves the home region in GDDB; for
    /// already-registered users the GDDB entry tells us where to ask. For
    /// brand-new signups (no GDDB entry yet), the caller must pass the
    /// target signup region in `region`.
    async fn auth_challenge(
        context: &Context,
        username: String,
        region: Option<String>,
    ) -> FieldResult<AuthChallenge> {
        if !USERNAME_REGEX.is_match(&username) {
            return Err(field_error("Invalid username"));
        }

        let user_org_id: ids::OrgId = username
            .parse()
            .map_err(|_| field_error("Invalid username"))?;

        let home = context
            .gddb_client
            .lookup_org(&user_org_id)
            .await
            .map_err(|e| {
                tracing::error!("Failed to look up user in GDDB: {e}");
                internal_error()
            })?;

        let target_region = match home {
            Some(h) => h,
            None => match region {
                Some(s) => parse_region_arg(&s)?,
                None => {
                    return Err(field_error("Region is required when signing up a new user"));
                }
            },
        };

        let mut ias_client = context.ias_for_region(&target_region).await?;
        let resp = ias_client
            .issue_challenge(ias::proto::IssueChallengeRequest {
                username: username.clone(),
            })
            .await
            .map_err(map_ias_status)?
            .into_inner();

        let challenge_string = resp.challenge;
        let taken = resp.user_taken;
        let webauthn_creds = resp.webauthn_credential_ids;

        let b64url = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let challenge_b64 = b64url.encode(challenge_string.as_bytes());

        let user_id_hash = sha2::Sha256::digest(username.as_bytes());
        let user_id_b64 = b64url.encode(user_id_hash);

        let exclude_credentials: Vec<serde_json::Value> = webauthn_creds
            .iter()
            .map(|cid| {
                serde_json::json!({
                    "type": "public-key",
                    "id": cid,
                })
            })
            .collect();

        let passkey_registration = JsonValue(serde_json::json!({
            "rp": {
                "id": *context.rp_id,
                "name": *context.rp_name,
            },
            "user": {
                "id": user_id_b64,
                "name": username,
                "displayName": username,
            },
            "challenge": challenge_b64,
            "pubKeyCredParams": [
                { "type": "public-key", "alg": -7 },
                { "type": "public-key", "alg": -8 },
            ],
            "timeout": 60000,
            "attestation": "none",
            "authenticatorSelection": {
                "residentKey": "preferred",
                "userVerification": "preferred",
            },
            "excludeCredentials": exclude_credentials,
        }));

        let passkey_signin = if !webauthn_creds.is_empty() {
            let allow_credentials: Vec<serde_json::Value> = webauthn_creds
                .iter()
                .map(|cid| {
                    serde_json::json!({
                        "type": "public-key",
                        "id": cid,
                    })
                })
                .collect();
            Some(JsonValue(serde_json::json!({
                "challenge": challenge_b64,
                "rpId": *context.rp_id,
                "timeout": 60000,
                "userVerification": "preferred",
                "allowCredentials": allow_credentials,
            })))
        } else {
            None
        };

        Ok(AuthChallenge {
            challenge: challenge_string,
            taken,
            passkey_registration,
            passkey_signin,
        })
    }

    async fn refresh_token(context: &Context) -> FieldResult<AuthSuccess> {
        let auth = context.check_auth().await?;
        let mut ias = context.ias_for_region(&auth.home_region).await?;
        let response = ias
            .refresh_token(ias::proto::RefreshTokenRequest {
                username: auth.username.clone(),
            })
            .await
            .map_err(map_ias_status)?
            .into_inner();
        let user = response.user.ok_or_else(|| {
            tracing::error!("IAS RefreshToken returned no user record");
            internal_error()
        })?;

        Ok(AuthSuccess {
            user: SignedInUser {
                user: UserData::from(user),
            },
            token: response.token,
        })
    }

    async fn organizations(context: &Context) -> FieldResult<Vec<Organization>> {
        let auth = context.check_auth().await?;
        let mut ias = context.ias_for_region(&auth.home_region).await?;
        let org_names = ias
            .list_user_orgs(ias::proto::ListUserOrgsRequest {
                username: auth.username.clone(),
            })
            .await
            .map_err(map_ias_status)?
            .into_inner()
            .org_names;

        let mut orgs: Vec<Organization> = org_names
            .into_iter()
            .filter_map(|name| {
                name.parse::<ids::OrgId>()
                    .ok()
                    .map(|id| Organization { name: id })
            })
            .collect();

        // Always include the user's own "personal org" (username)
        let personal_org = auth
            .username
            .parse::<ids::OrgId>()
            .map_err(|_| field_error("Invalid organization name"))?;
        if !orgs.iter().any(|o| o.name == personal_org) {
            orgs.insert(0, Organization { name: personal_org });
        }

        Ok(orgs)
    }

    async fn organization(context: &Context, name: String) -> FieldResult<Organization> {
        let auth = context.check_auth().await?;
        let org: ids::OrgId = name
            .parse()
            .map_err(|_| field_error("Invalid organization name"))?;

        let org_region = context.home_region_for_org(&org).await?;

        if org.as_str() != auth.username {
            let mut ias = context.ias_for_region(&org_region).await?;
            let is_member = ias
                .org_contains_member(ias::proto::OrgContainsMemberRequest {
                    name: org.to_string(),
                    username: auth.username.clone(),
                })
                .await
                .map_err(map_ias_status)?
                .into_inner()
                .value;
            if !is_member {
                return Err(field_error("Permission denied"));
            }
        }

        Ok(Organization { name: org })
    }
}

struct Mutation;

#[juniper::graphql_object(Context = Context)]
impl Mutation {
    async fn create_repository(
        context: &Context,
        organization: String,
        repository: String,
        #[graphql(name = "region")] region: Option<String>,
    ) -> FieldResult<Repository> {
        let auth = context.check_auth().await?;

        let org: ids::OrgId = organization
            .parse()
            .map_err(|_| field_error("Invalid organization name"))?;
        let repo: ids::RepoId = repository
            .parse()
            .map_err(|_| field_error("Invalid repository name"))?;
        let name = ids::RepoQid { org, repo };

        let org_region = context.home_region_for_org(&name.org).await?;

        let region = match region {
            Some(s) => parse_region_arg(&s)?,
            None => org_region.clone(),
        };

        if organization != auth.username {
            let mut ias = context.ias_for_region(&org_region).await?;
            let is_member = ias
                .org_contains_member(ias::proto::OrgContainsMemberRequest {
                    name: organization.clone(),
                    username: auth.username.clone(),
                })
                .await
                .map_err(map_ias_status)?
                .into_inner()
                .value;
            if !is_member {
                return Err(field_error("Permission denied"));
            }
        }

        if !USERNAME_REGEX.is_match(&repository) {
            return Err(field_error("Invalid repository name"));
        }

        context
            .gddb_client
            .reserve_repo(&name, &region)
            .await
            .map_err(|e| match e {
                gddb::ReserveError::NameTaken => field_error("Repository already exists"),
                _ => {
                    tracing::error!("Failed to reserve repo name in GDDB: {}", e);
                    internal_error()
                }
            })?;

        let repository = context
            .cdb_for_region(&region)
            .await?
            .repo(name)
            .create()
            .await
            .map_err(|e| match e {
                cdb::CreateRepositoryError::AlreadyExists => {
                    field_error("Repository already exists")
                }
                _ => {
                    tracing::error!("Failed to create repository: {}", e);
                    internal_error()
                }
            })?;

        Ok(Repository { repository })
    }

    async fn signup(
        context: &Context,
        username: String,
        email: String,
        proof: JsonValue,
        #[graphql(name = "region")] region: String,
        fullname: Option<String>,
    ) -> FieldResult<AuthSuccess> {
        if !USERNAME_REGEX.is_match(&username) {
            return Err(field_error("Invalid username"));
        }

        if !is_valid_email(&email) {
            return Err(field_error("Invalid email"));
        }

        let region = parse_region_arg(&region)?;

        // The username is also the user's personal-org name, so reserve it
        // in GDDB as an org name. This is the source of truth for global
        // name uniqueness (case-insensitive) and for routing.
        let user_org_id: ids::OrgId = username
            .parse()
            .map_err(|_| field_error("Invalid username"))?;
        context
            .gddb_client
            .reserve_org(&user_org_id, &region)
            .await
            .map_err(|e| match e {
                gddb::ReserveError::NameTaken => field_error("Username already taken"),
                _ => {
                    tracing::error!("Failed to reserve username in GDDB: {}", e);
                    internal_error()
                }
            })?;

        let proof_proto = proof_from_json(&proof, ProofKind::Registration)?;

        let mut ias = context.ias_for_region(&region).await?;
        let response = ias
            .signup(ias::proto::SignupRequest {
                username,
                email,
                fullname: fullname.filter(|s| !s.is_empty()),
                proof: Some(proof_proto),
            })
            .await
            .map_err(map_ias_status)?
            .into_inner();

        let user = response.user.ok_or_else(|| {
            tracing::error!("IAS Signup returned no user record");
            internal_error()
        })?;

        Ok(AuthSuccess {
            user: SignedInUser {
                user: UserData::from(user),
            },
            token: response.token,
        })
    }

    #[graphql(name = "updateFullname")]
    async fn update_fullname(context: &Context, fullname: String) -> FieldResult<SignedInUser> {
        let auth = context.check_auth().await?;
        let mut ias = context.ias_for_region(&auth.home_region).await?;
        let user = ias
            .update_fullname(ias::proto::UpdateFullnameRequest {
                username: auth.username.clone(),
                fullname,
            })
            .await
            .map_err(map_ias_status)?
            .into_inner();
        Ok(SignedInUser {
            user: UserData::from(user),
        })
    }

    #[graphql(name = "addPublicKey")]
    async fn add_public_key(context: &Context, proof: JsonValue) -> FieldResult<SignedInUser> {
        let auth = context.check_auth().await?;
        let proof_proto = proof_from_json(&proof, ProofKind::Registration)?;

        let mut ias = context.ias_for_region(&auth.home_region).await?;
        ias.add_credential(ias::proto::AddCredentialRequest {
            username: auth.username.clone(),
            proof: Some(proof_proto),
        })
        .await
        .map_err(map_ias_status)?;

        let user = ias
            .get_user(ias::proto::GetUserRequest {
                username: auth.username.clone(),
            })
            .await
            .map_err(map_ias_status)?
            .into_inner();

        Ok(SignedInUser {
            user: UserData::from(user),
        })
    }

    #[graphql(name = "removePublicKey")]
    async fn remove_public_key(
        context: &Context,
        fingerprint: String,
    ) -> FieldResult<SignedInUser> {
        let auth = context.check_auth().await?;
        let mut ias = context.ias_for_region(&auth.home_region).await?;
        ias.remove_credential(ias::proto::RemoveCredentialRequest {
            username: auth.username.clone(),
            fingerprint,
        })
        .await
        .map_err(map_ias_status)?;

        let user = ias
            .get_user(ias::proto::GetUserRequest {
                username: auth.username.clone(),
            })
            .await
            .map_err(map_ias_status)?
            .into_inner();

        Ok(SignedInUser {
            user: UserData::from(user),
        })
    }

    async fn signin(
        context: &Context,
        username: String,
        proof: JsonValue,
    ) -> FieldResult<AuthSuccess> {
        if !USERNAME_REGEX.is_match(&username) {
            return Err(field_error("Invalid username"));
        }

        let user_region = context.home_region_for_user(&username).await.map_err(|_| {
            // Don't leak whether the user exists vs. exists-elsewhere.
            field_error("Invalid credentials")
        })?;

        let proof_proto = proof_from_json(&proof, ProofKind::Assertion)?;
        let mut ias = context.ias_for_region(&user_region).await?;
        let response = ias
            .signin(ias::proto::SigninRequest {
                username,
                proof: Some(proof_proto),
            })
            .await
            .map_err(map_ias_status)?
            .into_inner();

        let user = response.user.ok_or_else(|| {
            tracing::error!("IAS Signin returned no user record");
            internal_error()
        })?;

        Ok(AuthSuccess {
            user: SignedInUser {
                user: UserData::from(user),
            },
            token: response.token,
        })
    }

    #[graphql(name = "createOrganization")]
    async fn create_organization(
        context: &Context,
        name: String,
        #[graphql(name = "region")] region: Option<String>,
    ) -> FieldResult<Organization> {
        let auth = context.check_auth().await?;

        if !USERNAME_REGEX.is_match(&name) {
            return Err(field_error("Invalid organization name"));
        }

        // Default a new org to the caller's home region (where their UDB
        // record already lives). Operators can override per-org by passing
        // `region:`.
        let region = match region {
            Some(s) => parse_region_arg(&s)?,
            None => auth.home_region.clone(),
        };

        let org_id: ids::OrgId = name
            .parse()
            .map_err(|_| field_error("Invalid organization name"))?;

        context
            .gddb_client
            .reserve_org(&org_id, &region)
            .await
            .map_err(|e| match e {
                gddb::ReserveError::NameTaken => field_error("Name already taken"),
                _ => {
                    tracing::error!("Failed to reserve org name in GDDB: {}", e);
                    internal_error()
                }
            })?;

        let mut ias = context.ias_for_region(&region).await?;
        ias.create_org(ias::proto::CreateOrgRequest {
            name: org_id.to_string(),
            creator: auth.username.clone(),
        })
        .await
        .map_err(map_ias_status)?;

        Ok(Organization { name: org_id })
    }

    #[graphql(name = "addOrganizationMember")]
    async fn add_organization_member(
        context: &Context,
        organization: String,
        username: String,
    ) -> FieldResult<Organization> {
        let auth = context.check_auth().await?;

        let org_id: ids::OrgId = organization
            .parse()
            .map_err(|_| field_error("Invalid organization name"))?;

        let org_region = context.home_region_for_org(&org_id).await?;
        let mut ias = context.ias_for_region(&org_region).await?;

        let is_member = ias
            .org_contains_member(ias::proto::OrgContainsMemberRequest {
                name: org_id.to_string(),
                username: auth.username.clone(),
            })
            .await
            .map_err(map_ias_status)?
            .into_inner()
            .value;
        if !is_member {
            return Err(field_error("Permission denied"));
        }

        ias.add_org_member(ias::proto::AddOrgMemberRequest {
            name: org_id.to_string(),
            username,
        })
        .await
        .map_err(map_ias_status)?;

        Ok(Organization { name: org_id })
    }

    #[graphql(name = "leaveOrganization")]
    async fn leave_organization(context: &Context, organization: String) -> FieldResult<bool> {
        let auth = context.check_auth().await?;

        let org_id: ids::OrgId = organization
            .parse()
            .map_err(|_| field_error("Invalid organization name"))?;

        if org_id.as_str() == auth.username {
            return Err(field_error("Cannot leave your own personal organization"));
        }

        let org_region = context.home_region_for_org(&org_id).await?;
        let mut ias = context.ias_for_region(&org_region).await?;

        let is_member = ias
            .org_contains_member(ias::proto::OrgContainsMemberRequest {
                name: org_id.to_string(),
                username: auth.username.clone(),
            })
            .await
            .map_err(map_ias_status)?
            .into_inner()
            .value;
        if !is_member {
            return Err(field_error("Not a member of this organization"));
        }

        ias.remove_org_member(ias::proto::RemoveOrgMemberRequest {
            name: org_id.to_string(),
            username: auth.username.clone(),
        })
        .await
        .map_err(map_ias_status)?;

        Ok(true)
    }

    /// Create a new deployment for the given commit hash and make it
    /// `Desired`, superseding whichever deployment is currently active
    /// in the same environment.
    ///
    /// Requires the caller to be a member of the owning organisation (the
    /// same access level as other repository-scoped operations).
    #[graphql(name = "createDeployment")]
    async fn create_deployment(
        context: &Context,
        organization: String,
        repository: String,
        environment: String,
        commit_hash: String,
    ) -> FieldResult<Deployment> {
        let auth = context.check_auth().await?;

        let org: ids::OrgId = organization
            .parse()
            .map_err(|_| field_error("Invalid organization name"))?;
        let repo: ids::RepoId = repository
            .parse()
            .map_err(|_| field_error("Invalid repository name"))?;
        let env: ids::EnvironmentId = environment
            .parse()
            .map_err(|_| field_error("Invalid environment name"))?;
        let commit: ids::ObjId = commit_hash
            .parse()
            .map_err(|_| field_error("Invalid commit hash"))?;
        let repo_qid = ids::RepoQid {
            org: org.clone(),
            repo,
        };

        let repo_region = context.home_region_for_repo(&repo_qid).await?;
        let org_region = context.home_region_for_org(&org).await?;

        if organization != auth.username {
            let mut ias = context.ias_for_region(&org_region).await?;
            let is_member = ias
                .org_contains_member(ias::proto::OrgContainsMemberRequest {
                    name: organization.clone(),
                    username: auth.username.clone(),
                })
                .await
                .map_err(map_ias_status)?
                .into_inner()
                .value;
            if !is_member {
                return Err(field_error("Permission denied"));
            }
        }

        let nonce = ids::DeploymentNonce::random();
        let deployment_id = ids::DeploymentId::new(commit, nonce);
        let client = context
            .cdb_for_region(&repo_region)
            .await?
            .repo(repo_qid)
            .deployment(env, deployment_id);

        client.make_desired().await.map_err(|e| {
            tracing::error!("Failed to create deployment: {}", e);
            internal_error()
        })?;

        let deployment = client.get().await.map_err(|e| {
            tracing::error!("Failed to load deployment after creation: {}", e);
            internal_error()
        })?;

        Ok(Deployment { deployment })
    }

    /// Tear down an environment by transitioning all currently-`Desired`
    /// deployments to `Undesired` without superseding them.  This mirrors
    /// the behaviour of deleting a Git ref via SCS.
    ///
    /// Requires the caller to be a member of the owning organisation.
    #[graphql(name = "tearDownEnvironment")]
    async fn tear_down_environment(
        context: &Context,
        organization: String,
        repository: String,
        environment: String,
    ) -> FieldResult<bool> {
        let auth = context.check_auth().await?;

        let org: ids::OrgId = organization
            .parse()
            .map_err(|_| field_error("Invalid organization name"))?;
        let repo: ids::RepoId = repository
            .parse()
            .map_err(|_| field_error("Invalid repository name"))?;
        let env: ids::EnvironmentId = environment
            .parse()
            .map_err(|_| field_error("Invalid environment name"))?;
        let repo_qid = ids::RepoQid {
            org: org.clone(),
            repo,
        };

        let repo_region = context.home_region_for_repo(&repo_qid).await?;
        let org_region = context.home_region_for_org(&org).await?;

        if organization != auth.username {
            let mut ias = context.ias_for_region(&org_region).await?;
            let is_member = ias
                .org_contains_member(ias::proto::OrgContainsMemberRequest {
                    name: organization.clone(),
                    username: auth.username.clone(),
                })
                .await
                .map_err(map_ias_status)?
                .into_inner()
                .value;
            if !is_member {
                return Err(field_error("Permission denied"));
            }
        }

        let repo_client = context.cdb_for_region(&repo_region).await?.repo(repo_qid);
        let mut stream = repo_client.active_deployments().await.map_err(|e| {
            tracing::error!("Failed to list active deployments: {}", e);
            internal_error()
        })?;

        while let Some(dep) = stream.next().await {
            let dep = dep.map_err(|e| {
                tracing::error!("Failed to read active deployment: {}", e);
                internal_error()
            })?;
            if dep.environment == env && dep.state == cdb::DeploymentState::Desired {
                let dc = repo_client.deployment(dep.environment.clone(), dep.deployment.clone());
                dc.set(cdb::DeploymentState::Undesired).await.map_err(|e| {
                    tracing::error!("Failed to set deployment to Undesired: {}", e);
                    internal_error()
                })?;
            }
        }

        Ok(true)
    }

    /// Manually request the deletion of a single resource.  Publishes a
    /// `Destroy` message to RTQ that the RTE worker pool will consume and
    /// forward to the resource's plugin.  The RDB row is cleared on the
    /// plugin's success; on failure the row remains and the failure is
    /// visible in the resource's log stream.
    ///
    /// This is an imperative action intended as an escape hatch.  The
    /// declarative model still applies: if the owning deployment is still
    /// `Desired` and the resource is part of its current evaluation, the
    /// deployment engine will recreate the resource on its next tick.
    ///
    /// Requires the caller to be a member of the owning organisation (the
    /// same access level as other repository-scoped mutations).
    #[graphql(name = "deleteResource")]
    async fn delete_resource(
        context: &Context,
        organization: String,
        repository: String,
        environment: String,
        resource: String,
    ) -> FieldResult<bool> {
        let auth = context.check_auth().await?;

        let org: ids::OrgId = organization
            .parse()
            .map_err(|_| field_error("Invalid organization name"))?;
        let repo: ids::RepoId = repository
            .parse()
            .map_err(|_| field_error("Invalid repository name"))?;
        let env: ids::EnvironmentId = environment
            .parse()
            .map_err(|_| field_error("Invalid environment name"))?;
        let resource_id: ids::ResourceId = resource
            .parse()
            .map_err(|_| field_error("Invalid resource ID"))?;

        let repo_qid = ids::RepoQid {
            org: org.clone(),
            repo,
        };
        let repo_region = context.home_region_for_repo(&repo_qid).await?;
        let org_region = context.home_region_for_org(&org).await?;

        if organization != auth.username {
            let mut ias = context.ias_for_region(&org_region).await?;
            let is_member = ias
                .org_contains_member(ias::proto::OrgContainsMemberRequest {
                    name: organization.clone(),
                    username: auth.username.clone(),
                })
                .await
                .map_err(map_ias_status)?
                .into_inner()
                .value;
            if !is_member {
                return Err(field_error("Permission denied"));
            }
        }

        let env_qid = ids::EnvironmentQid::new(repo_qid, env);
        let namespace = env_qid.to_string();

        let row = context
            .rdb_for_region(resource_id.region())
            .await?
            .namespace(namespace.clone())
            .resource(
                resource_id.resource_type().to_string(),
                resource_id.resource_name().to_string(),
            )
            .get()
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to read resource {resource_id} in namespace {namespace}: {e}"
                );
                internal_error()
            })?
            .ok_or_else(|| field_error("Resource not found"))?;

        let owner_str = row
            .owner
            .as_deref()
            .ok_or_else(|| field_error("Resource has no owner; cannot request deletion"))?;
        let owner_qid: ids::DeploymentQid = owner_str.parse().map_err(|_| {
            tracing::error!("Invalid owner deployment QID on resource row: {owner_str}");
            internal_error()
        })?;

        let resource_qid = ids::ResourceQid::new(env_qid.clone(), resource_id.clone());
        let resource_ref = rtq::ResourceRef {
            environment_qid: env_qid,
            resource_id,
        };

        let target_region = resource_ref.resource_id.region().clone();
        if let Ok(publisher) = context.ldb_publisher_for_region(&target_region).await
            && let Ok(namespace) = publisher.namespace(resource_qid.to_string()).await
        {
            namespace
                .info(format!("Manual deletion requested by {}", auth.username))
                .await;
        }

        let message = rtq::Message::Destroy(rtq::DestroyMessage {
            resource: resource_ref,
            deployment_id: owner_qid.deployment,
            home_region: repo_region,
        });

        context
            .rtq_publisher
            .enqueue(&target_region, &message)
            .await
            .map_err(|e| {
                tracing::error!("Failed to publish destroy message: {e}");
                internal_error()
            })?;

        Ok(true)
    }
}

pub(crate) type Schema = RootNode<'static, Query, Mutation, subscriptions::Subscription>;

fn schema() -> Schema {
    Schema::new(Query, Mutation, subscriptions::Subscription)
}

fn extract_bearer_token(headers: &http::header::HeaderMap) -> Option<String> {
    headers
        .get(http::header::AUTHORIZATION)
        .and_then(|h| h.as_bytes().strip_prefix(b"Bearer "))
        .and_then(|v| String::from_utf8(v.to_vec()).ok())
}

/// Bundle of auth-related extensions, kept together so the GraphQL HTTP
/// handlers stay under axum's 16-extractor limit.
#[derive(Clone)]
struct AuthState {
    ias_pool: pools::IasPool,
    region_keys: region_keys::RegionKeyCache,
}

/// Outcome of authenticating a bearer token. Distinguishes legitimate
/// rejection (`Invalid` / `Expired`) from infrastructure trouble
/// (`Internal`).
pub(crate) enum AuthOutcome {
    Authenticated(AuthenticatedUser),
    Invalid,
    Expired,
    Internal,
}

/// Authenticate a bearer token. Tokens are signed identity envelopes
/// (see the `auth_token` crate); the verifier looks up the issuer
/// region's public key via the [`region_keys::RegionKeyCache`] (which
/// pulls from the issuer region's IAS on cache miss), validates the
/// signature and expiry, and returns an [`AuthenticatedUser`] carrying
/// the username and home region claimed by the token.
pub(crate) async fn authenticate_token(
    token: &str,
    region_keys: &region_keys::RegionKeyCache,
) -> AuthOutcome {
    let unverified = match auth_token::parse(token) {
        Ok(u) => u,
        Err(_) => return AuthOutcome::Invalid,
    };
    let issuer = unverified.issuer_region().clone();
    let key = match region_keys.get(&issuer).await {
        Ok(key) => key,
        Err(region_keys::FetchError::Unknown(_)) => return AuthOutcome::Invalid,
        Err(e) => {
            tracing::error!("Failed to fetch region key for {issuer}: {e}");
            return AuthOutcome::Internal;
        }
    };
    let claims = match unverified.verify(&key) {
        Ok(claims) => claims,
        Err(auth_token::VerifyError::Expired) => return AuthOutcome::Expired,
        Err(auth_token::VerifyError::BadSignature) => {
            // Possibly stale cache. Drop the entry, refetch, and retry
            // once. Any failure on the retry is a real bad signature.
            region_keys.invalidate(&issuer).await;
            let fresh_key = match region_keys.get(&issuer).await {
                Ok(key) => key,
                Err(region_keys::FetchError::Unknown(_)) => return AuthOutcome::Invalid,
                Err(e) => {
                    tracing::error!("Failed to refetch region key for {issuer}: {e}");
                    return AuthOutcome::Internal;
                }
            };
            let reparsed = auth_token::parse(token).expect("parsed once already");
            match reparsed.verify(&fresh_key) {
                Ok(claims) => claims,
                Err(auth_token::VerifyError::Expired) => return AuthOutcome::Expired,
                Err(_) => return AuthOutcome::Invalid,
            }
        }
        Err(_) => return AuthOutcome::Invalid,
    };
    AuthOutcome::Authenticated(AuthenticatedUser {
        username: claims.username,
        home_region: claims.issuer_region,
    })
}

#[derive(Parser, Debug)]
#[command(name = "api", about = "Skyr GraphQL API")]
struct Cli {
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long, default_value_t = 8080)]
    port: u16,
    #[arg(long, default_value = "http://127.0.0.1:9000")]
    adb_endpoint_url: String,
    #[arg(long)]
    adb_external_url: Option<String>,
    #[arg(long, default_value = "skyr-artifacts")]
    adb_bucket: String,
    #[arg(long, env = "SKYR_ADB_ACCESS_KEY_ID")]
    adb_access_key_id: String,
    #[arg(long, env = "SKYR_ADB_SECRET_ACCESS_KEY")]
    adb_secret_access_key: String,
    /// S3 region used by the ADB client. Unrelated to the Skyr `--region`
    /// (this is the cloud-vendor region for the S3 endpoint).
    #[arg(long, default_value = "us-east-1")]
    adb_region: String,
    #[arg(long, default_value = "skyr.cloud")]
    rp_id: String,
    #[arg(long, default_value = "Skyr")]
    rp_name: String,
    #[arg(long)]
    write_schema: bool,
    /// Template used to construct region-scoped Skyr peer service
    /// addresses. Substitutes `{service}` (required) and `{region}`
    /// (optional). Defaults to `{service}.{region}.int.skyr.cloud` —
    /// override per stack (e.g. `{service}.<namespace>.svc.cluster.local`
    /// for a single-region Kubernetes deployment).
    ///
    /// The API edge does not need its own region — it routes per-data-piece
    /// using token claims, GDDB lookups, and region-prefixed resource IDs.
    /// The GDDB session below is bootstrapped against an arbitrary region's
    /// GDDB DNS name; in production every region's GDDB Scylla peer answers
    /// the same keyspace, so the choice is just a bootstrap detail.
    #[arg(long, default_value_t = ids::ServiceAddressTemplate::default_template())]
    service_address_template: ids::ServiceAddressTemplate,
    /// Optional region to bootstrap the GDDB Scylla session against. Used
    /// only as the region substituted into `--service-address-template` for
    /// the initial known-node address. GDDB is logically global; the Scylla
    /// session discovers the rest of the cluster from there.
    #[arg(long, default_value = "loca")]
    gddb_bootstrap_region: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    if cli.write_schema {
        let schema_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("schema.graphql");
        std::fs::write(&schema_path, schema().as_sdl())?;
        tracing::info!("wrote GraphQL schema to {}", schema_path.display());
        return Ok(());
    }

    let template = cli.service_address_template;
    let gddb_bootstrap_region: ids::RegionId = cli
        .gddb_bootstrap_region
        .parse()
        .map_err(|e: ids::ParseIdError| anyhow::anyhow!("invalid --gddb-bootstrap-region: {e}"))?;

    let ias_pool = pools::IasPool::new(template.clone());
    let cdb_pool = pools::CdbPool::new(template.clone());
    let sdb_pool = pools::SdbPool::new(template.clone());

    let gddb_client = gddb::ClientBuilder::new()
        .known_node(template.format("gddb", &gddb_bootstrap_region))
        .build()
        .await?;

    let rdb_pool = pools::RdbPool::new(template.clone());
    let mut adb_builder = adb::ClientBuilder::new()
        .bucket(cli.adb_bucket)
        .endpoint_url(cli.adb_endpoint_url)
        .region(cli.adb_region)
        .access_key_id(cli.adb_access_key_id)
        .secret_access_key(cli.adb_secret_access_key)
        .create_bucket_if_missing(true);
    if let Some(adb_external_url) = cli.adb_external_url {
        adb_builder = adb_builder.external_url(adb_external_url);
    }
    let adb_client = adb_builder.build().await?;
    let ldb_consumer_pool = pools::LdbConsumerPool::new(template.clone());
    let ldb_publisher_pool = pools::LdbPublisherPool::new(template.clone());
    let rtq_publisher = rtq::Publisher::new(template.clone());
    let region_key_cache = region_keys::RegionKeyCache::new(ias_pool.clone());
    let rp_id = Arc::new(cli.rp_id);
    let rp_name = Arc::new(cli.rp_name);

    let schema = Arc::new(schema());

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/graphql", get(graphql_ws_handler).post(graphql_handler))
        .route("/graphiql", get(graphiql))
        .layer(cors)
        .layer(Extension(schema))
        .layer(Extension(rp_id))
        .layer(Extension(rp_name))
        .layer(Extension(cdb_pool))
        .layer(Extension(sdb_pool))
        .layer(Extension(gddb_client))
        .layer(Extension(rdb_pool))
        .layer(Extension(adb_client))
        .layer(Extension(ldb_consumer_pool))
        .layer(Extension(ldb_publisher_pool))
        .layer(Extension(rtq_publisher))
        .layer(Extension(AuthState {
            ias_pool,
            region_keys: region_key_cache,
        }));

    let bind_target = format!("{}:{}", cli.host, cli.port);
    let addr = tokio::net::lookup_host(&bind_target)
        .await?
        .next()
        .ok_or_else(|| anyhow::anyhow!("failed to resolve bind address {bind_target}"))?;
    tracing::info!("listening on http://{addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

#[axum::debug_handler]
#[allow(clippy::too_many_arguments)]
async fn graphql_handler(
    Extension(schema): Extension<Arc<Schema>>,
    Extension(rp_id): Extension<Arc<String>>,
    Extension(rp_name): Extension<Arc<String>>,
    Extension(cdb_pool): Extension<pools::CdbPool>,
    Extension(sdb_pool): Extension<pools::SdbPool>,
    Extension(gddb_client): Extension<gddb::Client>,
    Extension(rdb_pool): Extension<pools::RdbPool>,
    Extension(adb_client): Extension<adb::Client>,
    Extension(ldb_consumer_pool): Extension<pools::LdbConsumerPool>,
    Extension(ldb_publisher_pool): Extension<pools::LdbPublisherPool>,
    Extension(rtq_publisher): Extension<rtq::Publisher>,
    Extension(auth): Extension<AuthState>,
    headers: http::header::HeaderMap,
    AxumJson(request): AxumJson<juniper::http::GraphQLRequest>,
) -> AxumJson<juniper::http::GraphQLResponse> {
    let AuthState {
        ias_pool,
        region_keys,
    } = auth;
    let authenticated_user = if let Some(token) = extract_bearer_token(&headers) {
        match authenticate_token(&token, &region_keys).await {
            AuthOutcome::Authenticated(user) => Some(user),
            AuthOutcome::Invalid | AuthOutcome::Expired => {
                return AxumJson(juniper::http::GraphQLResponse::error(
                    juniper::FieldError::new(
                        "Invalid token",
                        juniper::graphql_value!({ "code": "INVALID_TOKEN" }),
                    ),
                ));
            }
            AuthOutcome::Internal => {
                return AxumJson(juniper::http::GraphQLResponse::error(
                    "Internal server error".into(),
                ));
            }
        }
    } else {
        None
    };

    let ctx = Context {
        ias_pool,
        cdb_pool,
        sdb_pool,
        ldb_consumer_pool,
        ldb_publisher_pool,
        gddb_client,
        rdb_pool,
        adb_client,
        rtq_publisher,
        rp_id,
        rp_name,
        authenticated_user,
    };
    AxumJson(request.execute(&schema, &ctx).await)
}

async fn graphiql() -> Html<String> {
    Html(juniper::http::graphiql::graphiql_source(
        "/graphql",
        Some("/graphql"),
    ))
}

#[axum::debug_handler]
#[allow(clippy::too_many_arguments)]
async fn graphql_ws_handler(
    ws: WebSocketUpgrade,
    Extension(schema): Extension<Arc<Schema>>,
    Extension(rp_id): Extension<Arc<String>>,
    Extension(rp_name): Extension<Arc<String>>,
    Extension(cdb_pool): Extension<pools::CdbPool>,
    Extension(sdb_pool): Extension<pools::SdbPool>,
    Extension(gddb_client): Extension<gddb::Client>,
    Extension(rdb_pool): Extension<pools::RdbPool>,
    Extension(adb_client): Extension<adb::Client>,
    Extension(ldb_consumer_pool): Extension<pools::LdbConsumerPool>,
    Extension(ldb_publisher_pool): Extension<pools::LdbPublisherPool>,
    Extension(rtq_publisher): Extension<rtq::Publisher>,
    Extension(auth): Extension<AuthState>,
    headers: http::header::HeaderMap,
) -> Response {
    let AuthState {
        ias_pool,
        region_keys,
    } = auth;
    let authenticated_user = if let Some(token) = extract_bearer_token(&headers) {
        match authenticate_token(&token, &region_keys).await {
            AuthOutcome::Authenticated(user) => Some(user),
            AuthOutcome::Invalid | AuthOutcome::Expired => {
                return (StatusCode::UNAUTHORIZED, "Invalid token").into_response();
            }
            AuthOutcome::Internal => {
                return (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
                    .into_response();
            }
        }
    } else {
        None
    };

    let region_keys_for_ws = region_keys.clone();
    let context = Context {
        ias_pool,
        cdb_pool,
        sdb_pool,
        ldb_consumer_pool,
        ldb_publisher_pool,
        gddb_client,
        rdb_pool,
        adb_client,
        rtq_publisher,
        rp_id,
        rp_name,
        authenticated_user,
    };

    ws.protocols(["graphql-transport-ws"])
        .on_upgrade(move |socket| {
            graphql_ws::graphql_ws_connection(socket, schema, context, region_keys_for_ws)
        })
}

use std::sync::Arc;
use std::time::Duration;

mod auth;
mod challenge;
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
use chrono::Utc;
use clap::Parser;
use futures_util::StreamExt;
use http::StatusCode;
use juniper::{FieldResult, RootNode};
use sha2::Digest;
use tower_http::cors::{Any, CorsLayer};

use json_scalar::JsonValue;
use schema::{AuthChallenge, AuthSuccess, Deployment, Organization, Repository, SignedInUser};

/// How long a freshly-minted identity token is valid. Matches the legacy
/// opaque-token TTL (24h); shortening this requires shipping a refresh
/// flow that doesn't depend on the user being able to reach their home
/// region edge on every refresh.
const IDENTITY_TOKEN_TTL: Duration = Duration::from_secs(86400);

#[derive(Clone)]
pub(crate) struct Context {
    pub(crate) udb_pool: pools::UdbPool,
    pub(crate) cdb_pool: pools::CdbPool,
    pub(crate) sdb_pool: pools::SdbPool,
    pub(crate) gddb_client: gddb::Client,
    pub(crate) rdb_client: rdb::Client,
    pub(crate) adb_client: adb::Client,
    pub(crate) ldb_consumer: ldb::Consumer,
    pub(crate) ldb_publisher: ldb::Publisher,
    pub(crate) rtq_publisher: rtq::Publisher,
    pub(crate) challenger: Arc<challenge::Challenger>,
    pub(crate) rp_id: Arc<String>,
    pub(crate) rp_name: Arc<String>,
    pub(crate) user: Option<udb::UserClient>,
    /// The region this API binary can sign identity tokens for. The
    /// signing keypair attached to its UDB pool is valid only for users
    /// whose home region matches this value, so signup is constrained to
    /// it (the freshly-issued token would otherwise be unverifiable
    /// elsewhere). Routing-side decisions are no longer pinned to this
    /// region — see `home_region_for_*` for cross-region GDDB-driven
    /// routing. Goes away when UDB becomes its own service.
    pub(crate) region: ids::RegionId,
}

impl Context {
    pub(crate) async fn check_auth(&self) -> FieldResult<(udb::UserClient, udb::User)> {
        let err = field_error("Not authenticated");

        let Some(client) = self.user.clone() else {
            return Err(err);
        };

        let user = client.get().await.map_err(|e| {
            tracing::error!("Failed to fetch user: {}", e);
            err
        })?;

        Ok((client, user))
    }

    /// Resolve `org`'s home region via GDDB. Returns whatever region GDDB
    /// has on file — the API is now a true edge and routes per-data-piece
    /// (cross-region) instead of pinning everything to its own region.
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

    pub(crate) async fn udb_for_region(&self, region: &ids::RegionId) -> FieldResult<udb::Client> {
        self.udb_pool.for_region(region).await.map_err(|e| {
            tracing::error!("Failed to connect to UDB in {}: {}", region, e);
            internal_error()
        })
    }

    pub(crate) async fn cdb_for_region(&self, region: &ids::RegionId) -> FieldResult<cdb::Client> {
        self.cdb_pool.for_region(region).await.map_err(|e| {
            tracing::error!("Failed to connect to CDB in {}: {}", region, e);
            internal_error()
        })
    }

    pub(crate) async fn sdb_for_region(&self, region: &ids::RegionId) -> FieldResult<sdb::Client> {
        self.sdb_pool.for_region(region).await.map_err(|e| {
            tracing::error!("Failed to connect to SDB in {}: {}", region, e);
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

/// Parse an explicit `region` mutation argument. Any operator-known
/// region is accepted — the API no longer pins requests to its own
/// signing region. Callers that omit the argument supply their own
/// default (typically the caller's home region for create-org and
/// create-repository, or required outright for signup).
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
    // Domain must contain at least one dot with non-empty parts on each side
    let Some((domain_name, tld)) = domain.rsplit_once('.') else {
        return false;
    };
    if domain_name.is_empty() || tld.is_empty() {
        return false;
    }
    // Reject multiple @ signs
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
        let _ = (&context.cdb_pool, &context.rdb_client, &context.adb_client);
        tokio::task::yield_now().await;
        true
    }

    async fn me(context: &Context) -> FieldResult<SignedInUser> {
        let (_, user) = context.check_auth().await?;

        Ok(SignedInUser { user })
    }

    async fn auth_challenge(context: &Context, username: String) -> FieldResult<AuthChallenge> {
        if !USERNAME_REGEX.is_match(&username) {
            return Err(field_error("Invalid username"));
        }

        let b64url = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let challenge_string = context.challenger.challenge(Utc::now(), &username);
        let challenge_b64 = b64url.encode(challenge_string.as_bytes());

        let user_id_hash = sha2::Sha256::digest(username.as_bytes());
        let user_id_b64 = b64url.encode(user_id_hash);

        // Look up existing WebAuthn credentials for excludeCredentials /
        // allowCredentials. The user may not be registered yet (signup
        // flow), in which case GDDB returns None and we treat them as
        // having no credentials.
        let user_client = match username.parse::<ids::OrgId>() {
            Ok(parsed) => match context.gddb_client.lookup_org(&parsed).await {
                Ok(Some(home)) => Some(context.udb_for_region(&home).await?.user(&username)),
                Ok(None) => None,
                Err(e) => {
                    tracing::error!("Failed to look up user in GDDB: {}", e);
                    return Err(internal_error());
                }
            },
            Err(_) => None,
        };
        let (taken, credentials) = match &user_client {
            Some(c) => match c.get().await {
                Ok(_) => (
                    true,
                    c.pubkeys().list_credentials().await.unwrap_or_default(),
                ),
                Err(_) => (false, Vec::new()),
            },
            None => (false, Vec::new()),
        };

        let webauthn_creds: Vec<_> = credentials
            .iter()
            .filter_map(|c| c.credential_id.as_ref())
            .collect();

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
        let (_, user) = context.check_auth().await?;

        // Issuance lives on the user's home region UDB (it holds the
        // signing key for tokens claiming that issuer). Until UDB becomes
        // its own service that any API edge can RPC into, we can only
        // refresh tokens for users whose home matches our signing region.
        let user_region = context.home_region_for_user(&user.username).await?;
        if user_region != context.region {
            return Err(field_error("refresh from your home region's API edge"));
        }
        let user_client = context
            .udb_for_region(&user_region)
            .await?
            .user(&user.username);
        let token = user_client
            .issue_identity_token(IDENTITY_TOKEN_TTL)
            .map_err(|e| {
                tracing::error!("Failed to issue identity token: {}", e);
                internal_error()
            })?;

        Ok(AuthSuccess {
            user: SignedInUser { user },
            token,
        })
    }

    async fn organizations(context: &Context) -> FieldResult<Vec<Organization>> {
        let (_, user) = context.check_auth().await?;
        let user_region = context.home_region_for_user(&user.username).await?;
        let user_client = context
            .udb_for_region(&user_region)
            .await?
            .user(&user.username);

        let org_names = user_client.list_orgs().await.map_err(|e| {
            tracing::error!("Failed to list organizations: {}", e);
            internal_error()
        })?;

        let mut orgs: Vec<Organization> = org_names
            .into_iter()
            .filter_map(|name| {
                name.parse::<ids::OrgId>()
                    .ok()
                    .map(|id| Organization { name: id })
            })
            .collect();

        // Always include the user's own "personal org" (username)
        let personal_org = user
            .username
            .parse::<ids::OrgId>()
            .map_err(|_| field_error("Invalid organization name"))?;
        if !orgs.iter().any(|o| o.name == personal_org) {
            orgs.insert(0, Organization { name: personal_org });
        }

        Ok(orgs)
    }

    async fn organization(context: &Context, name: String) -> FieldResult<Organization> {
        let (_, user) = context.check_auth().await?;
        let org: ids::OrgId = name
            .parse()
            .map_err(|_| field_error("Invalid organization name"))?;

        let org_region = context.home_region_for_org(&org).await?;

        if org.as_str() != user.username {
            let is_member = context
                .udb_for_region(&org_region)
                .await?
                .org(org.as_str())
                .members()
                .contains(&user.username)
                .await
                .map_err(|e| {
                    tracing::error!("Failed to check org membership: {}", e);
                    internal_error()
                })?;
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
        let (_, user) = context.check_auth().await?;

        let org: ids::OrgId = organization
            .parse()
            .map_err(|_| field_error("Invalid organization name"))?;
        let repo: ids::RepoId = repository
            .parse()
            .map_err(|_| field_error("Invalid repository name"))?;
        let name = ids::RepoQid { org, repo };

        let org_region = context.home_region_for_org(&name.org).await?;

        // Default a new repo to its org's home region. Operators / users can
        // override per-repo by passing `region:`.
        let region = match region {
            Some(s) => parse_region_arg(&s)?,
            None => org_region.clone(),
        };

        if organization != user.username {
            let is_member = context
                .udb_for_region(&org_region)
                .await?
                .org(&organization)
                .members()
                .contains(&user.username)
                .await
                .map_err(|e| {
                    tracing::error!("Failed to check org membership: {}", e);
                    internal_error()
                })?;
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

        // Stage-5 caveat: an API edge can only mint identity tokens for
        // its own signing region. If a user signs up for a different home
        // region, the local UDB write would still succeed but the freshly
        // issued token couldn't be validated by other edges until they
        // refetched this region's GDDB pubkey. Until UDB becomes a service
        // (and any edge can RPC into the chosen region's UDB to sign), we
        // require the signup region to match this API's signing region.
        if region != context.region {
            return Err(field_error(
                "signup region must match this API's signing region",
            ));
        }

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

        let fullname = fullname.filter(|s| !s.is_empty());

        let now = Utc::now();

        let (openssh_key, credential_id, sign_count) =
            auth::verify_registration_proof(context, &proof, &username, now)?;

        let user_client = context.udb_for_region(&region).await?.user(&username);
        let user = match user_client.register(email, fullname).await {
            Err(udb::RegisterUserError::UsernameTaken) => {
                return Err(field_error("Username already taken"));
            }
            Err(udb::RegisterUserError::InvalidUsername(msg)) => {
                return Err(field_error(&format!("Invalid username: {msg}")));
            }
            Err(udb::RegisterUserError::InvalidEmail(msg)) => {
                return Err(field_error(&format!("Invalid email: {msg}")));
            }
            Err(e) => {
                tracing::error!("Failed to register user: {}", e);
                return Err(internal_error());
            }
            Ok(user) => user,
        };

        user_client
            .pubkeys()
            .add_credential(&openssh_key, credential_id.as_deref(), sign_count)
            .await
            .map_err(|e| {
                tracing::error!("Failed to add credential: {}", e);
                internal_error()
            })?;

        let token = user_client
            .issue_identity_token(IDENTITY_TOKEN_TTL)
            .map_err(|e| {
                tracing::error!("Failed to issue identity token: {}", e);
                internal_error()
            })?;

        Ok(AuthSuccess {
            user: SignedInUser { user },
            token,
        })
    }

    #[graphql(name = "updateFullname")]
    async fn update_fullname(context: &Context, fullname: String) -> FieldResult<SignedInUser> {
        let (user_client, _) = context.check_auth().await?;

        user_client.set_fullname(&fullname).await.map_err(|e| {
            tracing::error!("Failed to update fullname: {}", e);
            internal_error()
        })?;

        let user = user_client.get().await.map_err(|e| {
            tracing::error!("Failed to fetch user after fullname update: {}", e);
            internal_error()
        })?;

        Ok(SignedInUser { user })
    }

    #[graphql(name = "addPublicKey")]
    async fn add_public_key(context: &Context, proof: JsonValue) -> FieldResult<SignedInUser> {
        let (user_client, user) = context.check_auth().await?;
        let now = Utc::now();

        let (openssh_key, credential_id, sign_count) =
            auth::verify_registration_proof(context, &proof, &user.username, now)?;

        user_client
            .pubkeys()
            .add_credential(&openssh_key, credential_id.as_deref(), sign_count)
            .await
            .map_err(|e| {
                tracing::error!("Failed to add credential: {}", e);
                internal_error()
            })?;

        let user = user_client.get().await.map_err(|e| {
            tracing::error!("Failed to fetch user after adding public key: {}", e);
            internal_error()
        })?;

        Ok(SignedInUser { user })
    }

    #[graphql(name = "removePublicKey")]
    async fn remove_public_key(
        context: &Context,
        fingerprint: String,
    ) -> FieldResult<SignedInUser> {
        let (user_client, _) = context.check_auth().await?;

        user_client
            .pubkeys()
            .remove(&fingerprint)
            .await
            .map_err(|e| {
                tracing::error!("Failed to remove public key: {}", e);
                internal_error()
            })?;

        let user = user_client.get().await.map_err(|e| {
            tracing::error!("Failed to fetch user after removing public key: {}", e);
            internal_error()
        })?;

        Ok(SignedInUser { user })
    }

    async fn signin(
        context: &Context,
        username: String,
        proof: JsonValue,
    ) -> FieldResult<AuthSuccess> {
        if !USERNAME_REGEX.is_match(&username) {
            return Err(field_error("Invalid username"));
        }

        let now = Utc::now();

        let user_region = context.home_region_for_user(&username).await.map_err(|_| {
            // Don't leak whether the user exists vs. exists-elsewhere.
            field_error("Invalid credentials")
        })?;
        // Identity-token issuance only works for users in our signing
        // region; cross-region signin needs UDB-as-service (future work)
        // or anycast that routes to the user's home edge.
        if user_region != context.region {
            return Err(field_error("sign in from your home region's API edge"));
        }
        let user_client = context.udb_for_region(&user_region).await?.user(&username);
        let user = match user_client.get().await {
            Ok(user) => user,
            Err(udb::UserQueryError::NotFound) => {
                return Err(field_error("Invalid credentials"));
            }
            Err(e) => {
                tracing::error!("Failed to lookup user: {}", e);
                return Err(internal_error());
            }
        };

        match &proof.0 {
            serde_json::Value::String(sig_pem) => {
                // SSH signature flow
                auth::signin_ssh(context, &user_client, sig_pem, &username, now).await?;
            }
            serde_json::Value::Object(_) => {
                // WebAuthn assertion flow
                auth::signin_webauthn(context, &user_client, &proof.0, &username, now).await?;
            }
            _ => {
                return Err(field_error(
                    "Invalid proof: expected a string (SSH signature) or object (WebAuthn assertion)",
                ));
            }
        }

        let token = user_client
            .issue_identity_token(IDENTITY_TOKEN_TTL)
            .map_err(|e| {
                tracing::error!("Failed to issue identity token: {}", e);
                internal_error()
            })?;

        Ok(AuthSuccess {
            user: SignedInUser { user },
            token,
        })
    }

    #[graphql(name = "createOrganization")]
    async fn create_organization(
        context: &Context,
        name: String,
        #[graphql(name = "region")] region: Option<String>,
    ) -> FieldResult<Organization> {
        let (_, user) = context.check_auth().await?;

        if !USERNAME_REGEX.is_match(&name) {
            return Err(field_error("Invalid organization name"));
        }

        // Default a new org to the caller's home region (where their UDB
        // record already lives). Operators can override per-org by passing
        // `region:`.
        let user_region = context.home_region_for_user(&user.username).await?;
        let region = match region {
            Some(s) => parse_region_arg(&s)?,
            None => user_region,
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

        let org_client = context.udb_for_region(&region).await?.org(org_id.as_str());
        org_client
            .create(&user.username)
            .await
            .map_err(|e| match e {
                udb::CreateOrgError::NameTaken => field_error("Name already taken"),
                udb::CreateOrgError::InvalidName(msg) => {
                    field_error(&format!("Invalid name: {msg}"))
                }
                udb::CreateOrgError::CreatorNotFound => field_error("User not found"),
                _ => {
                    tracing::error!("Failed to create organization: {}", e);
                    internal_error()
                }
            })?;

        Ok(Organization { name: org_id })
    }

    #[graphql(name = "addOrganizationMember")]
    async fn add_organization_member(
        context: &Context,
        organization: String,
        username: String,
    ) -> FieldResult<Organization> {
        let (_, user) = context.check_auth().await?;

        let org_id: ids::OrgId = organization
            .parse()
            .map_err(|_| field_error("Invalid organization name"))?;

        let org_region = context.home_region_for_org(&org_id).await?;

        let org_client = context
            .udb_for_region(&org_region)
            .await?
            .org(org_id.as_str());

        // Verify the caller is a member of this org
        let is_member = org_client
            .members()
            .contains(&user.username)
            .await
            .map_err(|e| {
                tracing::error!("Failed to check org membership: {}", e);
                internal_error()
            })?;
        if !is_member {
            return Err(field_error("Permission denied"));
        }

        org_client
            .members()
            .add(&username)
            .await
            .map_err(|e| match e {
                udb::OrgQueryError::UserNotFound => field_error("User not found"),
                udb::OrgQueryError::AlreadyMember => field_error("User is already a member"),
                udb::OrgQueryError::NotFound => field_error("Organization not found"),
                _ => {
                    tracing::error!("Failed to add org member: {}", e);
                    internal_error()
                }
            })?;

        Ok(Organization { name: org_id })
    }

    #[graphql(name = "leaveOrganization")]
    async fn leave_organization(context: &Context, organization: String) -> FieldResult<bool> {
        let (_, user) = context.check_auth().await?;

        let org_id: ids::OrgId = organization
            .parse()
            .map_err(|_| field_error("Invalid organization name"))?;

        // Cannot leave your own personal org (username)
        if org_id.as_str() == user.username {
            return Err(field_error("Cannot leave your own personal organization"));
        }

        let org_region = context.home_region_for_org(&org_id).await?;

        let org_client = context
            .udb_for_region(&org_region)
            .await?
            .org(org_id.as_str());

        // Verify the caller is actually a member
        let is_member = org_client
            .members()
            .contains(&user.username)
            .await
            .map_err(|e| {
                tracing::error!("Failed to check org membership: {}", e);
                internal_error()
            })?;
        if !is_member {
            return Err(field_error("Not a member of this organization"));
        }

        org_client
            .members()
            .remove(&user.username)
            .await
            .map_err(|e| {
                tracing::error!("Failed to leave organization: {}", e);
                internal_error()
            })?;

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
        let (_, user) = context.check_auth().await?;

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
        // Org membership check uses the org's home (== repo's home for
        // personal orgs; in general the org's CDB record may be elsewhere).
        let org_region = context.home_region_for_org(&org).await?;

        // Access check: caller must be a member of the owning org (or it
        // must be their own personal org).
        if organization != user.username {
            let is_member = context
                .udb_for_region(&org_region)
                .await?
                .org(&organization)
                .members()
                .contains(&user.username)
                .await
                .map_err(|e| {
                    tracing::error!("Failed to check org membership: {}", e);
                    internal_error()
                })?;
            if !is_member {
                return Err(field_error("Permission denied"));
            }
        }

        // Each deployment gets a fresh nonce so that re-deploying the same
        // commit creates a distinct deployment identity.
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
        let (_, user) = context.check_auth().await?;

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

        if organization != user.username {
            let is_member = context
                .udb_for_region(&org_region)
                .await?
                .org(&organization)
                .members()
                .contains(&user.username)
                .await
                .map_err(|e| {
                    tracing::error!("Failed to check org membership: {}", e);
                    internal_error()
                })?;
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
        let (_, user) = context.check_auth().await?;

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

        if organization != user.username {
            let is_member = context
                .udb_for_region(&org_region)
                .await?
                .org(&organization)
                .members()
                .contains(&user.username)
                .await
                .map_err(|e| {
                    tracing::error!("Failed to check org membership: {}", e);
                    internal_error()
                })?;
            if !is_member {
                return Err(field_error("Permission denied"));
            }
        }

        let env_qid = ids::EnvironmentQid::new(repo_qid, env);
        let namespace = env_qid.to_string();

        let row = context
            .rdb_client
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

        // Emit an audit log before enqueueing so the action is visible
        // regardless of how the plugin-side delete resolves.  Published to
        // both the resource-QID and deployment-QID log topics with the
        // phrasing tailored to each: the resource topic already knows
        // which resource it is, so naming it would be redundant; the
        // deployment topic multiplexes many resources, so the line needs
        // to name the resource explicitly.
        if let Ok(publisher) = context
            .ldb_publisher
            .namespace(resource_qid.to_string())
            .await
        {
            publisher
                .info(format!("Manual deletion requested by {}", user.username))
                .await;
        }
        if let Ok(publisher) = context.ldb_publisher.namespace(owner_qid.to_string()).await {
            publisher
                .info(format!(
                    "Manual deletion of {}:{} requested by {}",
                    resource_ref.resource_type(),
                    resource_ref.resource_name(),
                    user.username,
                ))
                .await;
        }

        let target_region = resource_ref.resource_id.region().clone();
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
/// handlers stay under axum's 16-extractor limit. Each handler extracts
/// this once and pulls out the pieces it needs.
#[derive(Clone)]
struct AuthState {
    udb_pool: pools::UdbPool,
    region_keys: region_keys::RegionKeyCache,
    region: ids::RegionId,
}

/// Outcome of authenticating a bearer token. Distinguishes legitimate
/// rejection (`Invalid` / `Expired`) from infrastructure trouble
/// (`Internal`) so callers can render the right HTTP status.
pub(crate) enum AuthOutcome {
    Authenticated(udb::UserClient),
    Invalid,
    Expired,
    Internal,
}

/// Authenticate a bearer token, accepting either a signed identity token
/// (preferred) or a legacy opaque token stored in UDB Redis.
///
/// Identity tokens are tried first because they are self-validating and
/// cheaper (no Redis round-trip). On parse failure we fall back to the
/// legacy path so existing tokens continue to work during the rollout.
pub(crate) async fn authenticate_token(
    token: &str,
    udb_pool: &pools::UdbPool,
    region: &ids::RegionId,
    region_keys: &region_keys::RegionKeyCache,
) -> AuthOutcome {
    if let Ok(unverified) = auth_token::parse(token) {
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
        // The UserClient must read the user record from the user's home
        // region UDB. In stage 4 the API still rejects non-local home
        // regions elsewhere, so this is effectively the local UDB; in
        // stage 5 it routes per-user.
        let user_home = match udb_pool.for_region(&claims.issuer_region).await {
            Ok(client) => client,
            Err(e) => {
                tracing::error!("Failed to connect to UDB in {}: {e}", claims.issuer_region);
                return AuthOutcome::Internal;
            }
        };
        return AuthOutcome::Authenticated(user_home.user(claims.username));
    }

    // Legacy opaque-token path: tokens live in the local region's UDB
    // Redis. Identity tokens supersede this in stage 6.
    let local_udb = match udb_pool.for_region(region).await {
        Ok(client) => client,
        Err(e) => {
            tracing::error!("Failed to connect to local UDB: {e}");
            return AuthOutcome::Internal;
        }
    };
    match local_udb.lookup_token(token).await {
        Ok(user) => AuthOutcome::Authenticated(user),
        Err(udb::LookupTokenError::InvalidToken) => AuthOutcome::Invalid,
        Err(udb::LookupTokenError::Expired) => AuthOutcome::Expired,
        Err(e) => {
            tracing::error!("Failed to lookup opaque token: {e}");
            AuthOutcome::Internal
        }
    }
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
    #[arg(long, env = "SKYR_CHALLENGE_SALT")]
    challenge_salt: Option<String>,
    #[arg(long, default_value = "skyr.cloud")]
    rp_id: String,
    #[arg(long, default_value = "Skyr")]
    rp_name: String,
    #[arg(long)]
    write_schema: bool,
    /// The region this API binary can sign identity tokens for (e.g.
    /// `stockholm`). Validated as `[a-z]+`. Routing decisions no longer
    /// depend on this — every API edge is region-agnostic for reads and
    /// per-data-piece writes — but identity-token signing still requires
    /// access to a region's private key, so signup is constrained to this
    /// region until UDB becomes its own service.
    #[arg(long)]
    region: Option<String>,
    /// DNS suffix used to construct region-scoped Skyr peer service
    /// addresses. Combined with `--region`, peers are resolved as
    /// `<service>.<region>.int.<domain>` (e.g. `cdb.stockholm.int.skyr.cloud`).
    #[arg(long)]
    domain: String,
    /// Path to the 32-byte raw Ed25519 secret key used by this region's UDB
    /// to sign identity tokens. Generate with e.g.
    /// `head -c 32 /dev/urandom > udb-signing.key`. The corresponding
    /// public key is upserted into GDDB's `region_keys` table on startup.
    #[arg(long)]
    udb_signing_key: std::path::PathBuf,
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

    let challenge_salt = cli
        .challenge_salt
        .ok_or_else(|| anyhow::anyhow!("missing --challenge-salt"))?;

    let region: ids::RegionId = cli
        .region
        .ok_or_else(|| anyhow::anyhow!("missing --region"))?
        .parse()
        .map_err(|e: ids::ParseIdError| anyhow::anyhow!("invalid --region: {e}"))?;
    let domain: ids::Domain = cli
        .domain
        .parse()
        .map_err(|e: ids::ParseIdError| anyhow::anyhow!("invalid --domain: {e}"))?;

    let signing_identity = udb::SigningIdentity::load(region.clone(), &cli.udb_signing_key)
        .map_err(|e| anyhow::anyhow!("failed to load --udb-signing-key: {e}"))?;
    let signing_public_key = signing_identity.signing_key.verifying_key().to_bytes();

    let udb_pool = pools::UdbPool::new(domain.clone(), Some(signing_identity));
    let cdb_pool = pools::CdbPool::new(domain.clone());
    let sdb_pool = pools::SdbPool::new(domain.clone());

    let gddb_client = gddb::ClientBuilder::new()
        .known_node(ids::service_address("gddb", &region, &domain))
        .build()
        .await?;

    // Publish this region's identity-token public key into GDDB so other
    // regions' API edges can verify tokens we issue. Idempotent — safe to
    // re-run on every startup. `generation = 1` until rotation is
    // implemented.
    gddb_client
        .upsert_region_key(&region, &signing_public_key, 1)
        .await
        .map_err(|e| anyhow::anyhow!("failed to register UDB public key in GDDB: {e}"))?;

    let rdb_client = rdb::ClientBuilder::new()
        .known_node(ids::service_address("rdb", &region, &domain))
        .region(region.clone())
        .build()
        .await?;
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
    let ldb_brokers = format!("{}:9092", ids::service_address("ldb", &region, &domain));
    let ldb_consumer = ldb::ClientBuilder::new()
        .brokers(ldb_brokers.clone())
        .build_consumer()
        .await?;
    let ldb_publisher = ldb::ClientBuilder::new()
        .brokers(ldb_brokers)
        .build_publisher()
        .await?;
    let rtq_publisher = rtq::Publisher::new(domain.clone());
    let region_key_cache = region_keys::RegionKeyCache::new(gddb_client.clone());
    let challenger = Arc::new(challenge::Challenger::new(challenge_salt.into_bytes()));
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
        .layer(Extension(challenger))
        .layer(Extension(rp_id))
        .layer(Extension(rp_name))
        .layer(Extension(cdb_pool))
        .layer(Extension(sdb_pool))
        .layer(Extension(gddb_client))
        .layer(Extension(rdb_client))
        .layer(Extension(adb_client))
        .layer(Extension(ldb_consumer))
        .layer(Extension(ldb_publisher))
        .layer(Extension(rtq_publisher))
        .layer(Extension(AuthState {
            udb_pool,
            region_keys: region_key_cache,
            region,
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
    Extension(challenger): Extension<Arc<challenge::Challenger>>,
    Extension(rp_id): Extension<Arc<String>>,
    Extension(rp_name): Extension<Arc<String>>,
    Extension(cdb_pool): Extension<pools::CdbPool>,
    Extension(sdb_pool): Extension<pools::SdbPool>,
    Extension(gddb_client): Extension<gddb::Client>,
    Extension(rdb_client): Extension<rdb::Client>,
    Extension(adb_client): Extension<adb::Client>,
    Extension(ldb_consumer): Extension<ldb::Consumer>,
    Extension(ldb_publisher): Extension<ldb::Publisher>,
    Extension(rtq_publisher): Extension<rtq::Publisher>,
    Extension(auth): Extension<AuthState>,
    headers: http::header::HeaderMap,
    AxumJson(request): AxumJson<juniper::http::GraphQLRequest>,
) -> AxumJson<juniper::http::GraphQLResponse> {
    let AuthState {
        udb_pool,
        region_keys,
        region,
    } = auth;
    let user = if let Some(token) = extract_bearer_token(&headers) {
        match authenticate_token(&token, &udb_pool, &region, &region_keys).await {
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
        udb_pool,
        cdb_pool,
        sdb_pool,
        gddb_client,
        rdb_client,
        adb_client,
        ldb_consumer,
        ldb_publisher,
        rtq_publisher,
        challenger,
        rp_id,
        rp_name,
        user,
        region,
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
    Extension(challenger): Extension<Arc<challenge::Challenger>>,
    Extension(rp_id): Extension<Arc<String>>,
    Extension(rp_name): Extension<Arc<String>>,
    Extension(cdb_pool): Extension<pools::CdbPool>,
    Extension(sdb_pool): Extension<pools::SdbPool>,
    Extension(gddb_client): Extension<gddb::Client>,
    Extension(rdb_client): Extension<rdb::Client>,
    Extension(adb_client): Extension<adb::Client>,
    Extension(ldb_consumer): Extension<ldb::Consumer>,
    Extension(ldb_publisher): Extension<ldb::Publisher>,
    Extension(rtq_publisher): Extension<rtq::Publisher>,
    Extension(auth): Extension<AuthState>,
    headers: http::header::HeaderMap,
) -> Response {
    let AuthState {
        udb_pool,
        region_keys,
        region,
    } = auth;
    let user = if let Some(token) = extract_bearer_token(&headers) {
        match authenticate_token(&token, &udb_pool, &region, &region_keys).await {
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

    let udb_pool_for_ws = udb_pool.clone();
    let region_for_ws = region.clone();
    let region_keys_for_ws = region_keys.clone();
    let context = Context {
        udb_pool,
        cdb_pool,
        sdb_pool,
        gddb_client,
        rdb_client,
        adb_client,
        ldb_consumer,
        ldb_publisher,
        rtq_publisher,
        challenger,
        rp_id,
        rp_name,
        user,
        region,
    };

    ws.protocols(["graphql-transport-ws"])
        .on_upgrade(move |socket| {
            graphql_ws::graphql_ws_connection(
                socket,
                schema,
                context,
                udb_pool_for_ws,
                region_for_ws,
                region_keys_for_ws,
            )
        })
}

use std::{pin::Pin, sync::Arc, time::Duration};

mod challenge;

use axum::{
    Json as AxumJson, Router,
    extract::{
        Extension,
        ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
    },
    response::{Html, IntoResponse, Response},
    routing::get,
};
use chrono::{TimeZone, Utc};
use clap::Parser;
use futures_util::{Stream, StreamExt, TryStreamExt};
use http::StatusCode;
use juniper::{FieldResult, InputValue, RootNode, ScalarValue, Value, graphql_scalar};
use tower_http::cors::{Any, CorsLayer};

struct Context {
    udb_client: udb::Client,
    cdb_client: cdb::Client,
    rdb_client: rdb::Client,
    adb_client: adb::Client,
    ldb_brokers: String,
    challenger: Arc<challenge::Challenger>,
    user: Option<udb::UserClient>,
}

impl Context {
    async fn check_auth(&self) -> FieldResult<(udb::UserClient, udb::User)> {
        let err = field_error("Not authenticated");

        let Some(mut client) = self.user.clone() else {
            return Err(err);
        };

        let user = client.get().await.map_err(|e| {
            tracing::error!("Failed to fetch user: {}", e);
            err
        })?;

        Ok((client, user))
    }
}

impl juniper::Context for Context {}

fn field_error(message: &str) -> juniper::FieldError {
    juniper::FieldError::new(message, juniper::Value::Null)
}

fn internal_error() -> juniper::FieldError {
    field_error("Internal server error")
}

struct Query;

#[juniper::graphql_object(Context = Context)]
impl Query {
    async fn health(context: &Context) -> bool {
        let _ = (
            &context.cdb_client,
            &context.rdb_client,
            &context.adb_client,
        );
        tokio::task::yield_now().await;
        true
    }

    async fn me(context: &Context) -> FieldResult<User> {
        let (_, user) = context.check_auth().await?;

        Ok(User { user })
    }

    async fn auth_challenge(context: &Context, username: String) -> FieldResult<String> {
        if !USERNAME_REGEX.is_match(&username) {
            return Err(field_error("Invalid username"));
        }

        Ok(context.challenger.challenge(Utc::now(), &username))
    }

    async fn refresh_token(context: &Context) -> FieldResult<AuthSuccess> {
        let (user_client, user) = context.check_auth().await?;

        let token = user_client.tokens().issue().await.map_err(|e| {
            tracing::error!("Failed to issue token: {}", e);
            internal_error()
        })?;

        Ok(AuthSuccess {
            user: User { user },
            token,
        })
    }

    async fn organizations(context: &Context) -> FieldResult<Vec<Organization>> {
        let (_, user) = context.check_auth().await?;
        Ok(vec![Organization {
            name: user
                .username
                .parse::<ids::OrgId>()
                .map_err(|_| field_error("Invalid organization name"))?,
        }])
    }

    async fn organization(context: &Context, name: String) -> FieldResult<Organization> {
        let (_, _user) = context.check_auth().await?;
        let org: ids::OrgId = name
            .parse()
            .map_err(|_| field_error("Invalid organization name"))?;
        Ok(Organization { name: org })
    }

    async fn repositories(context: &Context) -> FieldResult<Vec<Repository>> {
        let (_, user) = context.check_auth().await?;
        context
            .cdb_client
            .repositories_by_organization(user.username.clone())
            .await
            .map_err(|e| {
                tracing::error!("Failed to list repositories: {}", e);
                internal_error()
            })?
            .map(|repository| repository.map(|repository| Repository { repository }))
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| {
                tracing::error!("Failed to read repository row: {}", e);
                internal_error()
            })
    }
}

struct Mutation;

static USERNAME_REGEX: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"^[a-zA-Z0-9_-]{3,20}$").unwrap());

#[juniper::graphql_object(Context = Context)]
impl Mutation {
    async fn create_repository(
        context: &Context,
        organization: String,
        repository: String,
    ) -> FieldResult<Repository> {
        let (_, user) = context.check_auth().await?;

        if organization != user.username {
            return Err(field_error("Permission denied"));
        }

        if !USERNAME_REGEX.is_match(&repository) {
            return Err(field_error("Invalid repository name"));
        }

        let org: ids::OrgId = organization
            .parse()
            .map_err(|_| field_error("Invalid organization name"))?;
        let repo: ids::RepoId = repository
            .parse()
            .map_err(|_| field_error("Invalid repository name"))?;
        let name = ids::RepoQid { org, repo };

        let repository = context
            .cdb_client
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
        pubkey: String,
        signature: String,
    ) -> FieldResult<AuthSuccess> {
        if !USERNAME_REGEX.is_match(&username) {
            return Err(field_error("Invalid username"));
        }

        if email.split('@').take(3).count() != 2 {
            return Err(field_error("Invalid email"));
        }

        let public_key =
            russh::keys::ssh_key::PublicKey::from_openssh(pubkey.trim()).map_err(|e| {
                tracing::warn!("Invalid pubkey format: {}", e);
                field_error("Invalid credentials")
            })?;
        context
            .challenger
            .check(&public_key, &signature, &username, Utc::now())
            .map_err(|_| field_error("Invalid credentials"))?;
        let pubkey_fingerprint = public_key.fingerprint(Default::default()).to_string();

        match context.udb_client.user(&username).register(email).await {
            Err(udb::RegisterUserError::UsernameTaken) => {
                Err(field_error("Username already taken"))
            }
            Err(e) => {
                tracing::error!("Failed to register user: {}", e);
                Err(internal_error())
            }
            Ok(user) => {
                context
                    .udb_client
                    .user(&username)
                    .pubkeys()
                    .add(pubkey_fingerprint)
                    .await
                    .map_err(|e| {
                        tracing::error!("Failed to add pubkey fingerprint: {}", e);
                        internal_error()
                    })?;

                let token = context
                    .udb_client
                    .user(&username)
                    .tokens()
                    .issue()
                    .await
                    .map_err(|e| {
                        tracing::error!("Failed to issue token: {}", e);
                        internal_error()
                    })?;

                Ok(AuthSuccess {
                    user: User { user },
                    token,
                })
            }
        }
    }

    async fn signin(
        context: &Context,
        username: String,
        signature: String,
        pubkey: String,
    ) -> FieldResult<AuthSuccess> {
        if !USERNAME_REGEX.is_match(&username) {
            return Err(field_error("Invalid username"));
        }

        let public_key =
            russh::keys::ssh_key::PublicKey::from_openssh(pubkey.trim()).map_err(|e| {
                tracing::warn!("Invalid pubkey format: {}", e);
                field_error("Invalid credentials")
            })?;
        let fingerprint = public_key.fingerprint(Default::default()).to_string();

        let mut user_client = context.udb_client.user(&username);
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

        let mut pubkeys = user_client.pubkeys();
        let has_fingerprint = pubkeys.contains(&fingerprint).await.map_err(|e| {
            tracing::error!("Failed to check pubkey fingerprint: {}", e);
            internal_error()
        })?;

        if !has_fingerprint {
            return Err(field_error("Invalid credentials"));
        }

        context
            .challenger
            .check(&public_key, &signature, &username, Utc::now())
            .map_err(|_| field_error("Invalid credentials"))?;

        let token = context
            .udb_client
            .user(&username)
            .tokens()
            .issue()
            .await
            .map_err(|e| {
                tracing::error!("Failed to issue token: {}", e);
                internal_error()
            })?;

        Ok(AuthSuccess {
            user: User { user },
            token,
        })
    }
}

struct AuthSuccess {
    user: User,
    token: String,
}

#[juniper::graphql_object(Context = Context)]
impl AuthSuccess {
    fn user(&self) -> &User {
        &self.user
    }

    fn token(&self) -> &str {
        &self.token
    }
}

struct User {
    user: udb::User,
}

#[juniper::graphql_object(Context = Context)]
impl User {
    fn username(&self) -> &str {
        &self.user.username
    }

    fn email(&self) -> &str {
        &self.user.email
    }

    fn fullname(&self) -> Option<&str> {
        self.user.fullname.as_deref()
    }
}

struct Organization {
    name: ids::OrgId,
}

#[juniper::graphql_object(Context = Context)]
impl Organization {
    fn name(&self) -> String {
        self.name.to_string()
    }

    async fn repository(&self, context: &Context, name: String) -> FieldResult<Repository> {
        let repo: ids::RepoId = name
            .parse()
            .map_err(|_| field_error("Invalid repository name"))?;
        let repo_qid = ids::RepoQid::new(self.name.clone(), repo);
        let repository = context
            .cdb_client
            .repository(&repo_qid)
            .await
            .map_err(|e| {
                tracing::error!("Failed to find repository {repo_qid}: {e}");
                internal_error()
            })?;
        Ok(Repository { repository })
    }

    async fn repositories(&self, context: &Context) -> FieldResult<Vec<Repository>> {
        context
            .cdb_client
            .repositories_by_organization(self.name.to_string())
            .await
            .map_err(|e| {
                tracing::error!("Failed to list repositories for {}: {}", self.name, e);
                internal_error()
            })?
            .map(|repository| repository.map(|repository| Repository { repository }))
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| {
                tracing::error!("Failed to read repository for {}: {}", self.name, e);
                internal_error()
            })
    }
}

struct Repository {
    repository: cdb::Repository,
}

#[juniper::graphql_object(Context = Context)]
impl Repository {
    fn organization(&self) -> Organization {
        Organization {
            name: self.repository.name.org.clone(),
        }
    }

    fn name(&self) -> String {
        self.repository.name.repo.to_string()
    }

    async fn environment(&self, context: &Context, name: String) -> FieldResult<Environment> {
        let env: ids::EnvironmentId = name
            .parse()
            .map_err(|_| field_error("Invalid environment name"))?;
        let qid = self.repository.name.environment(env);
        let deployments = context
            .cdb_client
            .repo(self.repository.name.clone())
            .deployments()
            .await
            .map_err(|e| {
                tracing::error!("Failed to list deployments for {qid}: {e}");
                internal_error()
            })?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| {
                tracing::error!("Failed to read deployments for {qid}: {e}");
                internal_error()
            })?;

        let deployments: Vec<_> = deployments
            .into_iter()
            .filter(|d| d.environment_qid() == qid)
            .collect();

        Ok(Environment { qid, deployments })
    }

    async fn environments(&self, context: &Context) -> FieldResult<Vec<Environment>> {
        let deployments = context
            .cdb_client
            .repo(self.repository.name.clone())
            .deployments()
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to list deployments for {}: {}",
                    self.repository.name,
                    e
                );
                internal_error()
            })?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to read deployments for {}: {}",
                    self.repository.name,
                    e
                );
                internal_error()
            })?;

        let mut env_map: std::collections::BTreeMap<String, Vec<cdb::Deployment>> =
            std::collections::BTreeMap::new();
        for deployment in deployments {
            let env_key = deployment.environment_qid().to_string();
            env_map.entry(env_key).or_default().push(deployment);
        }

        Ok(env_map
            .into_values()
            .map(|deployments| {
                let qid = deployments[0].environment_qid();
                Environment { qid, deployments }
            })
            .collect())
    }
}

struct Environment {
    qid: ids::EnvironmentQid,
    deployments: Vec<cdb::Deployment>,
}

#[juniper::graphql_object(Context = Context)]
impl Environment {
    fn name(&self) -> String {
        self.qid.environment.to_string()
    }

    fn qid(&self) -> String {
        self.qid.to_string()
    }

    async fn deployment(&self, context: &Context, commit: String) -> FieldResult<Deployment> {
        let deployment_id: ids::DeploymentId = commit
            .parse()
            .map_err(|_| field_error("Invalid commit hash"))?;
        let deployment = context
            .cdb_client
            .find_deployment(&self.qid.repo, &self.qid.environment, &deployment_id)
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to find deployment {deployment_id} in {}: {e}",
                    self.qid
                );
                internal_error()
            })?;
        Ok(Deployment { deployment })
    }

    fn deployments(&self) -> Vec<Deployment> {
        self.deployments
            .iter()
            .map(|deployment| Deployment {
                deployment: deployment.clone(),
            })
            .collect()
    }

    async fn resources(&self, context: &Context) -> FieldResult<Vec<Resource>> {
        let namespace = self.qid.to_string();

        context
            .rdb_client
            .namespace(namespace.clone())
            .list_resources()
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to list resources for environment namespace {namespace}: {e}"
                );
                internal_error()
            })?
            .map(|resource| resource.map(|resource| Resource { resource }))
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to load resources for environment namespace {namespace}: {e}"
                );
                internal_error()
            })
    }

    #[graphql(name = "lastLogs")]
    async fn last_logs(&self, context: &Context, amount: Option<i32>) -> FieldResult<Vec<Log>> {
        let amount = amount.unwrap_or(20).max(0) as u64;
        let mut all_logs = Vec::new();

        for deployment in &self.deployments {
            let deployment_qid = deployment.deployment_qid().to_string();
            match load_logs(context, deployment_qid.clone(), amount).await {
                Ok(logs) => all_logs.extend(logs),
                Err(error) => {
                    tracing::warn!("Failed to fetch logs for deployment {deployment_qid}: {error}");
                }
            }
        }

        all_logs.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
        all_logs.truncate(amount as usize);
        Ok(all_logs)
    }
}

struct Commit {
    repo: ids::RepoQid,
    hash: gix_hash::ObjectId,
    commit: gix_object::Commit,
}

#[juniper::graphql_object(Context = Context)]
impl Commit {
    fn hash(&self) -> String {
        self.hash.to_string()
    }

    fn message(&self) -> String {
        String::from_utf8_lossy(&self.commit.message).into_owned()
    }

    async fn tree(&self, context: &Context) -> FieldResult<Tree> {
        let repo_client = context.cdb_client.repo(self.repo.clone());
        let tree = repo_client.read_tree(self.commit.tree).await.map_err(|e| {
            tracing::error!("Failed to read tree {}: {e}", self.commit.tree);
            internal_error()
        })?;
        Ok(Tree {
            repo: self.repo.clone(),
            hash: self.commit.tree,
            name: None,
            tree,
        })
    }

    #[graphql(name = "treeEntry")]
    async fn tree_entry(&self, context: &Context, path: String) -> FieldResult<Option<TreeEntry>> {
        let repo_client = context.cdb_client.repo(self.repo.clone());
        let root_tree = repo_client.read_tree(self.commit.tree).await.map_err(|e| {
            tracing::error!("Failed to read root tree {}: {e}", self.commit.tree);
            internal_error()
        })?;

        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if segments.is_empty() {
            return Ok(Some(TreeEntry::Tree(Tree {
                repo: self.repo.clone(),
                hash: self.commit.tree,
                name: None,
                tree: root_tree,
            })));
        }

        let mut current_tree = root_tree;
        for (i, segment) in segments.iter().enumerate() {
            let entry = current_tree
                .entries
                .iter()
                .find(|e| e.filename.as_slice() == segment.as_bytes());

            let Some(entry) = entry else {
                return Ok(None);
            };

            if i == segments.len() - 1 {
                // Last segment: return the entry
                let name = Some(String::from_utf8_lossy(entry.filename.as_slice()).into_owned());
                if entry.mode.is_tree() {
                    let tree = repo_client.read_tree(entry.oid).await.map_err(|e| {
                        tracing::error!("Failed to read tree {}: {e}", entry.oid);
                        internal_error()
                    })?;
                    return Ok(Some(TreeEntry::Tree(Tree {
                        repo: self.repo.clone(),
                        hash: entry.oid,
                        name,
                        tree,
                    })));
                } else if entry.mode.is_blob() {
                    let blob = repo_client.read_blob(entry.oid).await.map_err(|e| {
                        tracing::error!("Failed to read blob {}: {e}", entry.oid);
                        internal_error()
                    })?;
                    return Ok(Some(TreeEntry::Blob(Blob {
                        hash: entry.oid,
                        name,
                        blob,
                    })));
                } else {
                    return Ok(None);
                }
            }

            // Intermediate segment: must be a tree
            if !entry.mode.is_tree() {
                return Ok(None);
            }
            current_tree = repo_client.read_tree(entry.oid).await.map_err(|e| {
                tracing::error!("Failed to read tree {}: {e}", entry.oid);
                internal_error()
            })?;
        }

        Ok(None)
    }
}

struct Tree {
    repo: ids::RepoQid,
    hash: gix_hash::ObjectId,
    name: Option<String>,
    tree: gix_object::Tree,
}

#[juniper::graphql_object(Context = Context)]
impl Tree {
    fn hash(&self) -> String {
        self.hash.to_string()
    }

    fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    async fn entries(&self, context: &Context) -> FieldResult<Vec<TreeEntry>> {
        let repo_client = context.cdb_client.repo(self.repo.clone());
        let mut entries = Vec::with_capacity(self.tree.entries.len());

        for entry in &self.tree.entries {
            let name = Some(String::from_utf8_lossy(entry.filename.as_slice()).into_owned());
            if entry.mode.is_tree() {
                let tree = repo_client.read_tree(entry.oid).await.map_err(|e| {
                    tracing::error!("Failed to read tree entry {}: {e}", entry.oid);
                    internal_error()
                })?;
                entries.push(TreeEntry::Tree(Tree {
                    repo: self.repo.clone(),
                    hash: entry.oid,
                    name,
                    tree,
                }));
            } else if entry.mode.is_blob() {
                let blob = repo_client.read_blob(entry.oid).await.map_err(|e| {
                    tracing::error!("Failed to read blob entry {}: {e}", entry.oid);
                    internal_error()
                })?;
                entries.push(TreeEntry::Blob(Blob {
                    hash: entry.oid,
                    name,
                    blob,
                }));
            }
            // Skip non-tree/non-blob entries (e.g., submodule commits)
        }

        Ok(entries)
    }
}

struct Blob {
    hash: gix_hash::ObjectId,
    name: Option<String>,
    blob: gix_object::Blob,
}

#[juniper::graphql_object(Context = Context)]
impl Blob {
    fn hash(&self) -> String {
        self.hash.to_string()
    }

    fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    fn size(&self) -> i32 {
        self.blob.data.len() as i32
    }

    fn content(&self) -> Option<String> {
        std::str::from_utf8(&self.blob.data).ok().map(String::from)
    }
}

#[derive(juniper::GraphQLUnion)]
#[graphql(Context = Context)]
enum TreeEntry {
    Blob(Blob),
    Tree(Tree),
}

struct Deployment {
    deployment: cdb::Deployment,
}

#[juniper::graphql_object(Context = Context)]
impl Deployment {
    fn id(&self) -> String {
        self.deployment.deployment_qid().to_string()
    }

    #[graphql(name = "ref")]
    fn r#ref(&self) -> String {
        self.deployment.environment.to_string()
    }

    async fn commit(&self, context: &Context) -> FieldResult<Commit> {
        let repo_client = context.cdb_client.repo(self.deployment.repo.clone());
        let hash = gix_hash::ObjectId::from_bytes_or_panic(&self.deployment.deployment.to_bytes());
        let commit = repo_client.read_commit(hash).await.map_err(|e| {
            tracing::error!("Failed to read commit {hash}: {e}");
            internal_error()
        })?;
        Ok(Commit {
            repo: self.deployment.repo.clone(),
            hash,
            commit,
        })
    }

    #[graphql(name = "createdAt")]
    fn created_at(&self) -> String {
        self.deployment.created_at.to_rfc3339()
    }

    fn state(&self) -> DeploymentState {
        self.deployment.state.into()
    }

    async fn resources(&self, context: &Context) -> FieldResult<Vec<Resource>> {
        let namespace = self.deployment.environment_qid().to_string();
        let owner = self.deployment.deployment_qid().to_string();

        context
            .rdb_client
            .namespace(namespace.clone())
            .list_resources_by_owner(&owner)
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to list resources for deployment namespace {namespace} and owner {owner}: {e}"
                );
                internal_error()
            })?
            .map(|resource| resource.map(|resource| Resource { resource }))
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to load resources for deployment namespace {namespace} and owner {owner}: {e}"
                );
                internal_error()
            })
    }

    async fn artifacts(&self, context: &Context) -> FieldResult<Vec<Artifact>> {
        let namespace = self.deployment.deployment_qid().to_string();
        let artifacts = context.adb_client.list(&namespace).await.map_err(|error| {
            tracing::error!("Failed to list artifacts for deployment {namespace}: {error}");
            internal_error()
        })?;

        Ok(artifacts
            .into_iter()
            .map(|header| Artifact { header })
            .collect())
    }

    #[graphql(name = "lastLogs")]
    async fn last_logs(&self, context: &Context, amount: Option<i32>) -> FieldResult<Vec<Log>> {
        let amount = amount.unwrap_or(20).max(0) as u64;
        let deployment_qid = self.deployment.deployment_qid().to_string();
        load_logs(context, deployment_qid.clone(), amount)
            .await
            .map_err(|error| {
                tracing::error!("Failed to fetch deployment logs for {deployment_qid}: {error}");
                internal_error()
            })
    }
}

struct Artifact {
    header: adb::ArtifactHeader,
}

#[juniper::graphql_object(Context = Context)]
impl Artifact {
    fn namespace(&self) -> &str {
        &self.header.namespace
    }

    fn name(&self) -> &str {
        &self.header.name
    }

    #[graphql(name = "mediaType")]
    fn media_type(&self) -> &str {
        &self.header.media_type
    }

    async fn url(&self, context: &Context) -> FieldResult<String> {
        context
            .adb_client
            .presign_read_url(
                &self.header.namespace,
                &self.header.name,
                Duration::from_secs(900),
            )
            .await
            .map_err(|error| {
                tracing::error!(
                    "Failed to presign artifact URL for {}/{}: {}",
                    self.header.namespace,
                    self.header.name,
                    error
                );
                internal_error()
            })
    }
}

#[derive(juniper::GraphQLObject)]
#[graphql(Context = Context)]
struct SourceFrame {
    #[graphql(name = "moduleId")]
    module_id: String,
    span: String,
    name: String,
}

struct Resource {
    resource: rdb::Resource,
}

impl Resource {
    fn resource_qid(&self) -> FieldResult<ids::ResourceQid> {
        let env_qid: ids::EnvironmentQid = self.resource.namespace.parse().map_err(|_| {
            tracing::error!(
                "Invalid environment QID in resource namespace: {}",
                self.resource.namespace
            );
            internal_error()
        })?;
        let resource_id = ids::ResourceId::new(&self.resource.resource_type, &self.resource.name);
        Ok(ids::ResourceQid::new(env_qid, resource_id))
    }
}

#[juniper::graphql_object(Context = Context)]
impl Resource {
    #[graphql(name = "type")]
    fn r#type(&self) -> &str {
        &self.resource.resource_type
    }

    fn name(&self) -> &str {
        &self.resource.name
    }

    fn inputs(&self) -> FieldResult<Option<JsonValue>> {
        self.resource
            .inputs
            .as_ref()
            .map(|record| {
                serde_json::to_value(record)
                    .map(JsonValue)
                    .map_err(|error| {
                        tracing::error!("Failed to serialize resource inputs to JSON: {error}");
                        internal_error()
                    })
            })
            .transpose()
    }

    fn outputs(&self) -> FieldResult<Option<JsonValue>> {
        self.resource
            .outputs
            .as_ref()
            .map(|record| {
                serde_json::to_value(record)
                    .map(JsonValue)
                    .map_err(|error| {
                        tracing::error!("Failed to serialize resource outputs to JSON: {error}");
                        internal_error()
                    })
            })
            .transpose()
    }

    async fn owner(&self, context: &Context) -> FieldResult<Option<Deployment>> {
        let Some(owner) = self.resource.owner.as_deref() else {
            return Ok(None);
        };

        let deployment_qid: ids::DeploymentQid = match owner.parse() {
            Ok(qid) => qid,
            Err(_) => {
                tracing::warn!("invalid resource owner deployment QID format: {owner}");
                return Ok(None);
            }
        };

        let repo_qid = deployment_qid.repo_qid().clone();

        let deployments = context
            .cdb_client
            .repo(repo_qid.clone())
            .deployments()
            .await
            .map_err(|e| {
                tracing::error!("Failed to list deployments for owner repository {repo_qid}: {e}");
                internal_error()
            })?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| {
                tracing::error!("Failed to read deployments for owner repository {repo_qid}: {e}");
                internal_error()
            })?;

        Ok(deployments
            .into_iter()
            .find(|deployment| deployment.deployment_qid().to_string() == owner)
            .map(|deployment| Deployment { deployment }))
    }

    async fn dependencies(&self, context: &Context) -> FieldResult<Vec<Resource>> {
        let mut dependencies = Vec::with_capacity(self.resource.dependencies.len());

        for dependency in &self.resource.dependencies {
            let resource = context
                .rdb_client
                .namespace(self.resource.namespace.clone())
                .resource(dependency.typ.clone(), dependency.name.clone())
                .get()
                .await
                .map_err(|error| {
                    tracing::error!(
                        "Failed to load dependency {dependency} in namespace {}: {error}",
                        self.resource.namespace,
                    );
                    internal_error()
                })?;

            if let Some(resource) = resource {
                dependencies.push(Resource { resource });
            }
        }

        Ok(dependencies)
    }

    fn markers(&self) -> Vec<ResourceMarker> {
        self.resource
            .markers
            .iter()
            .copied()
            .map(ResourceMarker::from)
            .collect()
    }

    #[graphql(name = "sourceTrace")]
    fn source_trace(&self) -> Vec<SourceFrame> {
        self.resource
            .source_trace
            .iter()
            .map(|f| SourceFrame {
                module_id: f.module_id.clone(),
                span: f.span.clone(),
                name: f.name.clone(),
            })
            .collect()
    }

    #[graphql(name = "lastLogs")]
    async fn last_logs(&self, context: &Context, amount: Option<i32>) -> FieldResult<Vec<Log>> {
        let amount = amount.unwrap_or(20).max(0) as u64;
        let resource_qid = self.resource_qid()?.to_string();
        load_logs(context, resource_qid.clone(), amount)
            .await
            .map_err(|error| {
                tracing::error!("Failed to fetch resource logs for {resource_qid}: {error}");
                internal_error()
            })
    }
}

#[derive(Clone, Copy, juniper::GraphQLEnum)]
enum DeploymentState {
    #[graphql(name = "DOWN")]
    Down,
    #[graphql(name = "UNDESIRED")]
    Undesired,
    #[graphql(name = "LINGERING")]
    Lingering,
    #[graphql(name = "DESIRED")]
    Desired,
    #[graphql(name = "UP")]
    Up,
}

impl From<cdb::DeploymentState> for DeploymentState {
    fn from(state: cdb::DeploymentState) -> Self {
        match state {
            cdb::DeploymentState::Down => DeploymentState::Down,
            cdb::DeploymentState::Undesired => DeploymentState::Undesired,
            cdb::DeploymentState::Lingering => DeploymentState::Lingering,
            cdb::DeploymentState::Desired => DeploymentState::Desired,
            cdb::DeploymentState::Up => DeploymentState::Up,
        }
    }
}

#[derive(Clone, Copy, juniper::GraphQLEnum)]
enum ResourceMarker {
    #[graphql(name = "VOLATILE")]
    Volatile,
    #[graphql(name = "STICKY")]
    Sticky,
}

impl From<sclc::Marker> for ResourceMarker {
    fn from(marker: sclc::Marker) -> Self {
        match marker {
            sclc::Marker::Volatile => ResourceMarker::Volatile,
            sclc::Marker::Sticky => ResourceMarker::Sticky,
        }
    }
}

#[derive(Clone, Copy, Debug, juniper::GraphQLEnum)]
enum Severity {
    #[graphql(name = "INFO")]
    Info,
    #[graphql(name = "WARNING")]
    Warning,
    #[graphql(name = "ERROR")]
    Err,
}

impl From<ldb::Severity> for Severity {
    fn from(severity: ldb::Severity) -> Self {
        match severity {
            ldb::Severity::Info => Severity::Info,
            ldb::Severity::Warning => Severity::Warning,
            ldb::Severity::Error => Severity::Err,
        }
    }
}

#[derive(Clone)]
struct Log {
    severity: Severity,
    timestamp: String,
    message: String,
}

#[juniper::graphql_object(Context = Context)]
impl Log {
    fn severity(&self) -> Severity {
        self.severity
    }

    fn timestamp(&self) -> &str {
        &self.timestamp
    }

    fn message(&self) -> &str {
        &self.message
    }
}

struct Subscription;

type LogStream = Pin<Box<dyn Stream<Item = Log> + Send>>;

#[juniper::graphql_subscription(Context = Context)]
impl Subscription {
    async fn deployment_logs(
        context: &Context,
        deployment_id: String,
        initial_amount: Option<i32>,
    ) -> FieldResult<LogStream> {
        let (_, user) = context.check_auth().await?;

        let deployment_qid: ids::DeploymentQid = deployment_id
            .parse()
            .map_err(|_| field_error("invalid deployment id"))?;
        let organization = deployment_qid.repo_qid().org.to_string();

        if organization != user.username {
            tracing::warn!(
                "Rejected deployment logs subscription for deployment outside user organization: deployment={} user={}",
                deployment_id,
                user.username
            );
            return Err(field_error("deployment outside user organization"));
        }

        let initial_amount = initial_amount.unwrap_or(1000).max(0) as u64;

        let consumer = ldb::ClientBuilder::new()
            .brokers(context.ldb_brokers.clone())
            .build_consumer()
            .await
            .map_err(|e| {
                tracing::error!("Failed to build ldb consumer for subscription: {}", e);
                field_error("failed to tail logs")
            })?;

        let namespace = consumer.namespace(deployment_id).await.map_err(|e| {
            tracing::error!("Failed to prepare deployment logs subscription consumer: {e}");
            field_error("failed to tail logs")
        })?;
        let mut inner = namespace
            .tail(ldb::TailConfig {
                follow: true,
                start_from: ldb::StartFrom::End(initial_amount),
            })
            .await
            .map_err(|e| {
                tracing::error!("Failed to tail deployment logs subscription: {e}");
                field_error("failed to tail logs")
            })?;

        Ok(Box::pin(async_stream::stream! {
            while let Some(item) = inner.next().await {
                match item {
                    Ok((timestamp, severity, message)) => {
                        yield Log {
                            severity: severity.into(),
                            timestamp: format_timestamp(timestamp),
                            message,
                        };
                    }
                    Err(error) => {
                        tracing::warn!("Error while streaming deployment logs: {}", error);
                        break;
                    }
                }
            }
        }))
    }

    async fn environment_logs(
        context: &Context,
        environment_qid: String,
        initial_amount: Option<i32>,
    ) -> FieldResult<LogStream> {
        let (_, user) = context.check_auth().await?;

        let env_qid: ids::EnvironmentQid = environment_qid
            .parse()
            .map_err(|_| field_error("invalid environment QID"))?;

        let organization = env_qid.repo.org.to_string();
        if organization != user.username {
            tracing::warn!(
                "Rejected environment logs subscription for environment outside user organization: environment={} user={}",
                environment_qid,
                user.username
            );
            return Err(field_error("environment outside user organization"));
        }

        let initial_amount = initial_amount.unwrap_or(1000).max(0) as u64;

        let consumer = ldb::ClientBuilder::new()
            .brokers(context.ldb_brokers.clone())
            .build_consumer()
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to build ldb consumer for environment logs subscription: {e}"
                );
                field_error("failed to tail logs")
            })?;

        let cdb_client = context.cdb_client.clone();

        Ok(Box::pin(async_stream::stream! {
            let mut merged = futures_util::stream::SelectAll::new();
            let mut subscribed: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
            let mut poll_interval = tokio::time::interval(Duration::from_secs(3));

            loop {
                tokio::select! {
                    biased;

                    Some(item) = merged.next(), if !merged.is_empty() => {
                        match item {
                            Ok((timestamp, severity, message)) => {
                                let severity = Severity::from(severity);
                                yield Log {
                                    severity,
                                    timestamp: format_timestamp(timestamp),
                                    message,
                                };
                            }
                            Err(error) => {
                                tracing::warn!("Error while streaming environment logs: {}", error);
                                break;
                            }
                        }
                    }

                    _ = poll_interval.tick() => {
                        let deployments = match cdb_client
                            .repo(env_qid.repo.clone())
                            .deployments()
                            .await
                        {
                            Ok(stream) => match stream.try_collect::<Vec<_>>().await {
                                Ok(deployments) => deployments,
                                Err(e) => {
                                    tracing::warn!("Failed to read deployments while polling for environment logs: {e}");
                                    continue;
                                }
                            },
                            Err(e) => {
                                tracing::warn!("Failed to list deployments while polling for environment logs: {e}");
                                continue;
                            }
                        };

                        for deployment in deployments {
                            if deployment.environment_qid() != env_qid {
                                continue;
                            }
                            let deployment_qid = deployment.deployment_qid().to_string();
                            if !subscribed.insert(deployment_qid.clone()) {
                                continue;
                            }

                            let namespace = match consumer.namespace(deployment_qid.clone()).await {
                                Ok(ns) => ns,
                                Err(e) => {
                                    tracing::warn!("Failed to prepare log consumer for deployment {deployment_qid}: {e}");
                                    subscribed.remove(&deployment_qid);
                                    continue;
                                }
                            };
                            match namespace
                                .tail(ldb::TailConfig {
                                    follow: true,
                                    start_from: ldb::StartFrom::End(initial_amount),
                                })
                                .await
                            {
                                Ok(stream) => {
                                    tracing::info!("Started log consumer for deployment {deployment_qid}");
                                    merged.push(stream);
                                }
                                Err(e) => {
                                    tracing::warn!("Failed to tail logs for deployment {deployment_qid}: {e}");
                                    subscribed.remove(&deployment_qid);
                                }
                            }
                        }
                    }
                }
            }
        }))
    }

    async fn resource_logs(
        context: &Context,
        resource_qid: String,
        initial_amount: Option<i32>,
    ) -> FieldResult<LogStream> {
        let (_, user) = context.check_auth().await?;

        let parsed_qid: ids::ResourceQid = resource_qid
            .parse()
            .map_err(|_| field_error("invalid resource QID"))?;
        let organization = parsed_qid.environment_qid().repo.org.to_string();

        if organization != user.username {
            tracing::warn!(
                "Rejected resource logs subscription for resource outside user organization: resource={} user={}",
                resource_qid,
                user.username
            );
            return Err(field_error("resource outside user organization"));
        }

        let initial_amount = initial_amount.unwrap_or(1000).max(0) as u64;

        let consumer = ldb::ClientBuilder::new()
            .brokers(context.ldb_brokers.clone())
            .build_consumer()
            .await
            .map_err(|e| {
                tracing::error!("Failed to build ldb consumer for resource logs subscription: {e}");
                field_error("failed to tail logs")
            })?;

        let namespace = consumer
            .namespace(resource_qid.clone())
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to prepare resource logs subscription consumer for {resource_qid}: {e}"
                );
                field_error("failed to tail logs")
            })?;
        let mut inner = namespace
            .tail(ldb::TailConfig {
                follow: true,
                start_from: ldb::StartFrom::End(initial_amount),
            })
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to tail resource logs subscription for {resource_qid}: {e}"
                );
                field_error("failed to tail logs")
            })?;

        Ok(Box::pin(async_stream::stream! {
            while let Some(item) = inner.next().await {
                match item {
                    Ok((timestamp, severity, message)) => {
                        yield Log {
                            severity: severity.into(),
                            timestamp: format_timestamp(timestamp),
                            message,
                        };
                    }
                    Err(error) => {
                        tracing::warn!("Error while streaming resource logs: {}", error);
                        break;
                    }
                }
            }
        }))
    }
}

async fn load_logs(context: &Context, namespace: String, amount: u64) -> anyhow::Result<Vec<Log>> {
    let consumer = ldb::ClientBuilder::new()
        .brokers(context.ldb_brokers.clone())
        .build_consumer()
        .await?;
    let namespace = consumer.namespace(namespace).await?;
    let mut stream = namespace
        .tail(ldb::TailConfig {
            follow: false,
            start_from: ldb::StartFrom::End(amount),
        })
        .await?;

    let mut logs = Vec::new();
    while let Some(item) = stream.next().await {
        let (timestamp, severity, message) = item?;
        logs.push(Log {
            severity: severity.into(),
            timestamp: format_timestamp(timestamp),
            message,
        });
    }

    Ok(logs)
}

fn format_timestamp(timestamp_millis: u64) -> String {
    let timestamp_millis = i64::try_from(timestamp_millis).unwrap_or(i64::MAX);
    Utc.timestamp_millis_opt(timestamp_millis)
        .single()
        .map(|timestamp| timestamp.to_rfc3339())
        .unwrap_or_else(|| String::from("9999-12-31T23:59:59.999+00:00"))
}

#[derive(Clone, Debug)]
#[graphql_scalar(with = json_scalar, parse_token(String), name = "JSON")]
struct JsonValue(serde_json::Value);

mod json_scalar {
    use super::*;

    pub(super) fn to_output<S: ScalarValue>(value: &JsonValue) -> Value<S> {
        json_to_graphql_value(&value.0)
    }

    pub(super) fn from_input<S: ScalarValue>(value: &InputValue<S>) -> Result<JsonValue, String> {
        Ok(JsonValue(input_to_json(value)?))
    }
}

fn json_to_graphql_value<S: ScalarValue>(value: &serde_json::Value) -> Value<S> {
    match value {
        serde_json::Value::Null => Value::null(),
        serde_json::Value::Bool(value) => Value::scalar(*value),
        serde_json::Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                if let Ok(value) = i32::try_from(value) {
                    Value::scalar(value)
                } else {
                    Value::scalar(value as f64)
                }
            } else if let Some(value) = value.as_u64() {
                if let Ok(value) = i32::try_from(value) {
                    Value::scalar(value)
                } else {
                    Value::scalar(value as f64)
                }
            } else if let Some(value) = value.as_f64() {
                Value::scalar(value)
            } else {
                Value::null()
            }
        }
        serde_json::Value::String(value) => Value::scalar(value.clone()),
        serde_json::Value::Array(values) => Value::list(
            values
                .iter()
                .map(json_to_graphql_value::<S>)
                .collect::<Vec<_>>(),
        ),
        serde_json::Value::Object(values) => {
            let mut object = juniper::Object::with_capacity(values.len());
            for (name, value) in values {
                object.add_field(name.to_string(), json_to_graphql_value::<S>(value));
            }
            Value::object(object)
        }
    }
}

fn input_to_json<S: ScalarValue>(value: &InputValue<S>) -> Result<serde_json::Value, String> {
    match value {
        InputValue::Null => Ok(serde_json::Value::Null),
        InputValue::Scalar(scalar) => {
            if let Some(value) = scalar.as_str() {
                Ok(serde_json::Value::String(value.to_string()))
            } else if let Some(value) = scalar.as_bool() {
                Ok(serde_json::Value::Bool(value))
            } else if let Some(value) = scalar.as_int() {
                Ok(serde_json::Value::Number(serde_json::Number::from(value)))
            } else if let Some(value) = scalar.as_float() {
                let Some(value) = serde_json::Number::from_f64(value) else {
                    return Err("JSON cannot represent NaN or infinite floats".to_string());
                };
                Ok(serde_json::Value::Number(value))
            } else {
                Err(format!("Expected JSON scalar, found: {value}"))
            }
        }
        InputValue::Enum(value) | InputValue::Variable(value) => {
            Ok(serde_json::Value::String(value.clone()))
        }
        InputValue::List(values) => {
            let mut array = Vec::with_capacity(values.len());
            for item in values {
                array.push(input_to_json(&item.item)?);
            }
            Ok(serde_json::Value::Array(array))
        }
        InputValue::Object(values) => {
            let mut object = serde_json::Map::with_capacity(values.len());
            for (key, item) in values {
                object.insert(key.item.clone(), input_to_json(&item.item)?);
            }
            Ok(serde_json::Value::Object(object))
        }
    }
}

fn serialize_execution_errors(
    errors: &[juniper::ExecutionError<juniper::DefaultScalarValue>],
) -> serde_json::Value {
    serde_json::to_value(errors).unwrap_or_else(|error| {
        serde_json::json!([{
            "message": format!("failed to serialize execution errors: {error}")
        }])
    })
}

fn graphql_value_to_json(value: &Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Scalar(scalar) => {
            if let Some(value) = scalar.as_str() {
                serde_json::Value::String(value.to_string())
            } else if let Some(value) = scalar.as_bool() {
                serde_json::Value::Bool(value)
            } else if let Some(value) = scalar.as_int() {
                serde_json::Value::Number(serde_json::Number::from(value))
            } else if let Some(value) = scalar.as_float() {
                serde_json::Number::from_f64(value)
                    .map(serde_json::Value::Number)
                    .unwrap_or(serde_json::Value::Null)
            } else {
                serde_json::Value::String(scalar.to_string())
            }
        }
        Value::List(values) => {
            serde_json::Value::Array(values.iter().map(graphql_value_to_json).collect::<Vec<_>>())
        }
        Value::Object(values) => {
            let mut object = serde_json::Map::with_capacity(values.field_count());
            for (name, value) in values.iter() {
                object.insert(name.to_string(), graphql_value_to_json(value));
            }
            serde_json::Value::Object(object)
        }
    }
}

type Schema = RootNode<'static, Query, Mutation, Subscription>;

fn schema() -> Schema {
    Schema::new(Query, Mutation, Subscription)
}

fn extract_bearer_token(headers: &http::header::HeaderMap) -> Option<String> {
    headers
        .get(http::header::AUTHORIZATION)
        .and_then(|h| h.as_bytes().strip_prefix(b"Bearer "))
        .and_then(|v| String::from_utf8(v.to_vec()).ok())
}

#[derive(Parser, Debug)]
#[command(name = "api", about = "Skyr GraphQL API")]
struct Cli {
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long, default_value_t = 8080)]
    port: u16,
    #[arg(long, default_value = "localhost")]
    cdb_hostname: String,
    #[arg(long, default_value = "localhost")]
    rdb_hostname: String,
    #[arg(long, default_value = "localhost")]
    udb_hostname: String,
    #[arg(long, default_value = "localhost")]
    ldb_hostname: String,
    #[arg(long, default_value = "http://127.0.0.1:9000")]
    adb_endpoint_url: String,
    #[arg(long)]
    adb_presign_endpoint_url: Option<String>,
    #[arg(long, default_value = "skyr-artifacts")]
    adb_bucket: String,
    #[arg(long, default_value = "minioadmin")]
    adb_access_key_id: String,
    #[arg(long, default_value = "minioadmin")]
    adb_secret_access_key: String,
    #[arg(long, default_value = "us-east-1")]
    adb_region: String,
    #[arg(long)]
    challenge_salt: Option<String>,
    #[arg(long)]
    write_schema: bool,
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

    let udb_client = udb::ClientBuilder::new()
        .known_node(cli.udb_hostname)
        .build()
        .await?;
    let cdb_client = cdb::ClientBuilder::new()
        .known_node(cli.cdb_hostname)
        .build()
        .await?;
    let rdb_client = rdb::ClientBuilder::new()
        .known_node(cli.rdb_hostname)
        .build()
        .await?;
    let mut adb_builder = adb::ClientBuilder::new()
        .bucket(cli.adb_bucket)
        .endpoint_url(cli.adb_endpoint_url)
        .region(cli.adb_region)
        .access_key_id(cli.adb_access_key_id)
        .secret_access_key(cli.adb_secret_access_key)
        .create_bucket_if_missing(true);
    if let Some(adb_presign_endpoint_url) = cli.adb_presign_endpoint_url {
        adb_builder = adb_builder.presign_endpoint_url(adb_presign_endpoint_url);
    }
    let adb_client = adb_builder.build().await?;
    let ldb_brokers = format!("{}:9092", cli.ldb_hostname);
    let challenger = Arc::new(challenge::Challenger::new(challenge_salt.into_bytes()));

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
        .layer(Extension(cdb_client))
        .layer(Extension(rdb_client))
        .layer(Extension(adb_client))
        .layer(Extension(ldb_brokers))
        .layer(Extension(udb_client));

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
    Extension(cdb_client): Extension<cdb::Client>,
    Extension(rdb_client): Extension<rdb::Client>,
    Extension(adb_client): Extension<adb::Client>,
    Extension(ldb_brokers): Extension<String>,
    Extension(mut udb_client): Extension<udb::Client>,
    headers: http::header::HeaderMap,
    AxumJson(request): AxumJson<juniper::http::GraphQLRequest>,
) -> AxumJson<juniper::http::GraphQLResponse> {
    let auth_header = extract_bearer_token(&headers);

    if let Some(token) = auth_header {
        match udb_client.lookup_token(token).await {
            Err(udb::LookupTokenError::InvalidToken) => {
                return AxumJson(juniper::http::GraphQLResponse::error(
                    "Invalid token".into(),
                ));
            }
            Err(e) => {
                tracing::error!("Failed to lookup token: {}", e);
                return AxumJson(juniper::http::GraphQLResponse::error(
                    "Internal server error".into(),
                ));
            }
            Ok(user) => {
                let ctx = Context {
                    udb_client,
                    cdb_client,
                    rdb_client,
                    adb_client,
                    ldb_brokers,
                    challenger,
                    user: Some(user),
                };
                return AxumJson(request.execute(&schema, &ctx).await);
            }
        }
    }

    let ctx = Context {
        udb_client,
        cdb_client,
        rdb_client,
        adb_client,
        ldb_brokers,
        challenger,
        user: None,
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
    Extension(cdb_client): Extension<cdb::Client>,
    Extension(rdb_client): Extension<rdb::Client>,
    Extension(adb_client): Extension<adb::Client>,
    Extension(ldb_brokers): Extension<String>,
    Extension(mut udb_client): Extension<udb::Client>,
    headers: http::header::HeaderMap,
) -> Response {
    let auth_header = extract_bearer_token(&headers);

    let user = if let Some(token) = auth_header {
        match udb_client.lookup_token(token).await {
            Err(udb::LookupTokenError::InvalidToken) => {
                return (StatusCode::UNAUTHORIZED, "Invalid token").into_response();
            }
            Err(e) => {
                tracing::error!("Failed to lookup token for websocket: {}", e);
                return (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
                    .into_response();
            }
            Ok(user) => Some(user),
        }
    } else {
        None
    };

    let udb_for_ws = udb_client.clone();
    let context = Context {
        udb_client,
        cdb_client,
        rdb_client,
        adb_client,
        ldb_brokers,
        challenger,
        user,
    };

    ws.protocols(["graphql-transport-ws"])
        .on_upgrade(move |socket| graphql_ws_connection(socket, schema, context, udb_for_ws))
}

async fn graphql_ws_connection(
    mut socket: WebSocket,
    schema: Arc<Schema>,
    mut context: Context,
    mut udb_client: udb::Client,
) {
    let mut initialized = false;

    while let Some(message) = socket.recv().await {
        let message = match message {
            Ok(message) => message,
            Err(error) => {
                tracing::warn!("GraphQL websocket receive error: {}", error);
                break;
            }
        };

        match message {
            WsMessage::Text(text) => {
                let payload: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(payload) => payload,
                    Err(error) => {
                        tracing::warn!("GraphQL websocket invalid json message: {}", error);
                        if !send_ws_json(
                            &mut socket,
                            serde_json::json!({
                                "type": "error",
                                "payload": [{
                                    "message": format!("invalid websocket message: {error}")
                                }]
                            }),
                        )
                        .await
                        {
                            break;
                        }
                        continue;
                    }
                };

                let Some(message_type) = payload
                    .get("type")
                    .and_then(|message_type| message_type.as_str())
                else {
                    if !send_ws_json(
                        &mut socket,
                        serde_json::json!({
                            "type": "error",
                            "payload": [{
                                "message": "missing websocket message type"
                            }]
                        }),
                    )
                    .await
                    {
                        break;
                    }
                    continue;
                };

                match message_type {
                    "connection_init" => {
                        // Support auth via connection_init payload for browser clients
                        // that cannot set custom HTTP headers on WebSocket upgrade.
                        if context.user.is_none()
                            && let Some(token) = payload
                                .get("payload")
                                .and_then(|p| p.get("Authorization"))
                                .and_then(|v| v.as_str())
                                .and_then(|v| v.strip_prefix("Bearer "))
                        {
                            match udb_client.lookup_token(token.to_owned()).await {
                                Ok(user) => {
                                    context.user = Some(user);
                                }
                                Err(udb::LookupTokenError::InvalidToken) => {
                                    tracing::debug!("Invalid token in connection_init payload");
                                }
                                Err(e) => {
                                    tracing::error!(
                                        "Failed to lookup token from connection_init: {}",
                                        e
                                    );
                                }
                            }
                        }

                        initialized = true;
                        if !send_ws_json(
                            &mut socket,
                            serde_json::json!({
                                "type": "connection_ack"
                            }),
                        )
                        .await
                        {
                            break;
                        }
                    }
                    "ping" => {
                        if !send_ws_json(
                            &mut socket,
                            serde_json::json!({
                                "type": "pong"
                            }),
                        )
                        .await
                        {
                            break;
                        }
                    }
                    "subscribe" => {
                        if !initialized {
                            if !send_ws_json(
                                &mut socket,
                                serde_json::json!({
                                    "type": "error",
                                    "payload": [{
                                        "message": "connection_init must be sent before subscribe"
                                    }]
                                }),
                            )
                            .await
                            {
                                break;
                            }
                            continue;
                        }

                        let Some(subscription_id) = payload
                            .get("id")
                            .and_then(|id| id.as_str())
                            .map(ToOwned::to_owned)
                        else {
                            if !send_ws_json(
                                &mut socket,
                                serde_json::json!({
                                    "type": "error",
                                    "payload": [{
                                        "message": "subscribe message missing id"
                                    }]
                                }),
                            )
                            .await
                            {
                                break;
                            }
                            continue;
                        };

                        let Some(request_payload) = payload.get("payload") else {
                            if !send_ws_json(
                                &mut socket,
                                serde_json::json!({
                                    "id": subscription_id,
                                    "type": "error",
                                    "payload": [{
                                        "message": "subscribe message missing payload"
                                    }]
                                }),
                            )
                            .await
                            {
                                break;
                            }
                            continue;
                        };

                        let request: juniper::http::GraphQLRequest =
                            match serde_json::from_value(request_payload.clone()) {
                                Ok(request) => request,
                                Err(error) => {
                                    if !send_ws_json(
                                    &mut socket,
                                    serde_json::json!({
                                        "id": subscription_id,
                                        "type": "error",
                                        "payload": [{
                                            "message": format!("invalid subscribe payload: {error}")
                                        }]
                                    }),
                                )
                                .await
                                {
                                    break;
                                }
                                    continue;
                                }
                            };

                        let stream_result =
                            juniper::http::resolve_into_stream(&request, &schema, &context).await;

                        let (subscription_value, initial_errors) = match stream_result {
                            Ok(result) => result,
                            Err(error) => {
                                if !send_ws_json(
                                    &mut socket,
                                    serde_json::json!({
                                        "id": subscription_id,
                                        "type": "error",
                                        "payload": [{
                                            "message": format!("{error}")
                                        }]
                                    }),
                                )
                                .await
                                {
                                    break;
                                }
                                continue;
                            }
                        };

                        if !initial_errors.is_empty() {
                            if !send_ws_json(
                                &mut socket,
                                serde_json::json!({
                                    "id": subscription_id,
                                    "type": "error",
                                    "payload": serialize_execution_errors(&initial_errors)
                                }),
                            )
                            .await
                            {
                                break;
                            }
                            continue;
                        }

                        let Some(fields) = subscription_value.into_object() else {
                            if !send_ws_json(
                                &mut socket,
                                serde_json::json!({
                                    "id": subscription_id,
                                    "type": "error",
                                    "payload": [{
                                        "message": "subscription did not return a stream field"
                                    }]
                                }),
                            )
                            .await
                            {
                                break;
                            }
                            continue;
                        };
                        let Some((field_name, field_value)) = fields.into_iter().next() else {
                            if !send_ws_json(
                                &mut socket,
                                serde_json::json!({
                                    "id": subscription_id,
                                    "type": "error",
                                    "payload": [{
                                        "message": "subscription did not return any stream fields"
                                    }]
                                }),
                            )
                            .await
                            {
                                break;
                            }
                            continue;
                        };
                        let Value::Scalar(mut stream) = field_value else {
                            if !send_ws_json(
                                &mut socket,
                                serde_json::json!({
                                    "id": subscription_id,
                                    "type": "error",
                                    "payload": [{
                                        "message": "subscription field was not a stream"
                                    }]
                                }),
                            )
                            .await
                            {
                                break;
                            }
                            continue;
                        };

                        while let Some(item) = stream.next().await {
                            let event = match item {
                                Ok(value) => serde_json::json!({
                                    "id": subscription_id,
                                    "type": "next",
                                    "payload": {
                                        "data": {
                                            field_name.clone(): graphql_value_to_json(&value)
                                        }
                                    }
                                }),
                                Err(error) => serde_json::json!({
                                    "id": subscription_id,
                                    "type": "next",
                                    "payload": {
                                        "errors": serialize_execution_errors(std::slice::from_ref(&error))
                                    }
                                }),
                            };

                            if !send_ws_json(&mut socket, event).await {
                                return;
                            }
                        }

                        if !send_ws_json(
                            &mut socket,
                            serde_json::json!({
                                "id": subscription_id,
                                "type": "complete"
                            }),
                        )
                        .await
                        {
                            return;
                        }
                    }
                    "complete" | "pong" => {}
                    other => {
                        if !send_ws_json(
                            &mut socket,
                            serde_json::json!({
                                "type": "error",
                                "payload": [{
                                    "message": format!("unsupported websocket message type: {other}")
                                }]
                            }),
                        )
                        .await
                        {
                            break;
                        }
                    }
                }
            }
            WsMessage::Ping(bytes) => {
                if socket.send(WsMessage::Pong(bytes)).await.is_err() {
                    break;
                }
            }
            WsMessage::Pong(_) => {}
            WsMessage::Close(_) => break,
            WsMessage::Binary(_) => {}
        }
    }
}

async fn send_ws_json(socket: &mut WebSocket, value: serde_json::Value) -> bool {
    socket
        .send(WsMessage::Text(value.to_string()))
        .await
        .is_ok()
}

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

pub struct Context {
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
        let err = juniper::FieldError::new("Not authenticated", juniper::Value::Null);

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

pub struct Query;

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
            return Err(juniper::FieldError::new(
                "Invalid username",
                juniper::Value::Null,
            ));
        }

        Ok(context.challenger.challenge(Utc::now(), &username))
    }

    async fn repositories(context: &Context) -> FieldResult<Vec<Repository>> {
        let (_, user) = context.check_auth().await?;
        context
            .cdb_client
            .repositories_by_organization(user.username.clone())
            .await
            .map_err(|e| {
                tracing::error!("Failed to list repositories: {}", e);
                juniper::FieldError::new("Internal server error", juniper::Value::Null)
            })?
            .map(|repository| repository.map(|repository| Repository { repository }))
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| {
                tracing::error!("Failed to read repository row: {}", e);
                juniper::FieldError::new("Internal server error", juniper::Value::Null)
            })
    }
}

pub struct Mutation;

lazy_static::lazy_static! {
    static ref USERNAME_REGEX: regex::Regex = regex::Regex::new(r"^[a-zA-Z0-9_-]{3,20}$").unwrap();
}

#[juniper::graphql_object(Context = Context)]
impl Mutation {
    async fn create_repository(
        context: &Context,
        organization: String,
        repository: String,
    ) -> FieldResult<Repository> {
        let (_, user) = context.check_auth().await?;

        if organization != user.username {
            return Err(juniper::FieldError::new(
                "Permission denied",
                juniper::Value::Null,
            ));
        }

        if !USERNAME_REGEX.is_match(&repository) {
            return Err(juniper::FieldError::new(
                "Invalid repository name",
                juniper::Value::Null,
            ));
        }

        let org: ids::OrgId = organization.parse().map_err(|_| {
            juniper::FieldError::new("Invalid organization name", juniper::Value::Null)
        })?;
        let repo: ids::RepoId = repository.parse().map_err(|_| {
            juniper::FieldError::new("Invalid repository name", juniper::Value::Null)
        })?;
        let name = ids::RepoQid { org, repo };

        let repository = context
            .cdb_client
            .repo(name)
            .create()
            .await
            .map_err(|e| match e {
                cdb::CreateRepositoryError::AlreadyExists => {
                    juniper::FieldError::new("Repository already exists", juniper::Value::Null)
                }
                _ => {
                    tracing::error!("Failed to create repository: {}", e);
                    juniper::FieldError::new("Internal server error", juniper::Value::Null)
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
            return Err(juniper::FieldError::new(
                "Invalid username",
                juniper::Value::Null,
            ));
        }

        if email.split('@').take(3).count() != 2 {
            return Err(juniper::FieldError::new(
                "Invalid email",
                juniper::Value::Null,
            ));
        }

        let public_key =
            russh::keys::ssh_key::PublicKey::from_openssh(pubkey.trim()).map_err(|e| {
                tracing::warn!("Invalid pubkey format: {}", e);
                juniper::FieldError::new("Invalid credentials", juniper::Value::Null)
            })?;
        context
            .challenger
            .check(&public_key, &signature, &username, Utc::now())
            .map_err(|_| juniper::FieldError::new("Invalid credentials", juniper::Value::Null))?;
        let pubkey_fingerprint = public_key.fingerprint(Default::default()).to_string();

        match context.udb_client.user(&username).register(email).await {
            Err(udb::RegisterUserError::UsernameTaken) => Err(juniper::FieldError::new(
                "Username already taken",
                juniper::Value::Null,
            )),
            Err(e) => {
                tracing::error!("Failed to register user: {}", e);
                Err(juniper::FieldError::new(
                    "Internal server error",
                    juniper::Value::Null,
                ))
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
                        juniper::FieldError::new("Internal server error", juniper::Value::Null)
                    })?;

                let token = context
                    .udb_client
                    .user(&username)
                    .tokens()
                    .issue()
                    .await
                    .map_err(|e| {
                        tracing::error!("Failed to issue token: {}", e);
                        juniper::FieldError::new("Internal server error", juniper::Value::Null)
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
            return Err(juniper::FieldError::new(
                "Invalid username",
                juniper::Value::Null,
            ));
        }

        let public_key =
            russh::keys::ssh_key::PublicKey::from_openssh(pubkey.trim()).map_err(|e| {
                tracing::warn!("Invalid pubkey format: {}", e);
                juniper::FieldError::new("Invalid credentials", juniper::Value::Null)
            })?;
        let fingerprint = public_key.fingerprint(Default::default()).to_string();

        let mut user_client = context.udb_client.user(&username);
        let user = match user_client.get().await {
            Ok(user) => user,
            Err(udb::UserQueryError::NotFound) => {
                return Err(juniper::FieldError::new(
                    "Invalid credentials",
                    juniper::Value::Null,
                ));
            }
            Err(e) => {
                tracing::error!("Failed to lookup user: {}", e);
                return Err(juniper::FieldError::new(
                    "Internal server error",
                    juniper::Value::Null,
                ));
            }
        };

        let mut pubkeys = user_client.pubkeys();
        let has_fingerprint = pubkeys.contains(&fingerprint).await.map_err(|e| {
            tracing::error!("Failed to check pubkey fingerprint: {}", e);
            juniper::FieldError::new("Internal server error", juniper::Value::Null)
        })?;

        if !has_fingerprint {
            return Err(juniper::FieldError::new(
                "Invalid credentials",
                juniper::Value::Null,
            ));
        }

        context
            .challenger
            .check(&public_key, &signature, &username, Utc::now())
            .map_err(|_| juniper::FieldError::new("Invalid credentials", juniper::Value::Null))?;

        let token = context
            .udb_client
            .user(&username)
            .tokens()
            .issue()
            .await
            .map_err(|e| {
                tracing::error!("Failed to issue token: {}", e);
                juniper::FieldError::new("Internal server error", juniper::Value::Null)
            })?;

        Ok(AuthSuccess {
            user: User { user },
            token,
        })
    }
}

pub struct AuthSuccess {
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

pub struct User {
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
        self.user.fullname.as_ref().map(|s| s.as_str())
    }
}

pub struct Repository {
    repository: cdb::Repository,
}

#[juniper::graphql_object(Context = Context)]
impl Repository {
    fn name(&self) -> String {
        self.repository.name.repo.to_string()
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
                juniper::FieldError::new("Internal server error", juniper::Value::Null)
            })?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to read deployments for {}: {}",
                    self.repository.name,
                    e
                );
                juniper::FieldError::new("Internal server error", juniper::Value::Null)
            })?;

        let mut env_map: std::collections::BTreeMap<String, Vec<cdb::Deployment>> =
            std::collections::BTreeMap::new();
        for deployment in deployments {
            let env_key = deployment.environment_qid().to_string();
            env_map.entry(env_key).or_default().push(deployment);
        }

        Ok(env_map
            .into_iter()
            .map(|(_, deployments)| {
                let qid = deployments[0].environment_qid();
                Environment { qid, deployments }
            })
            .collect())
    }
}

pub struct Environment {
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
                juniper::FieldError::new("Internal server error", juniper::Value::Null)
            })?
            .map(|resource| resource.map(|resource| Resource { resource }))
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to load resources for environment namespace {namespace}: {e}"
                );
                juniper::FieldError::new("Internal server error", juniper::Value::Null)
            })
    }

    #[graphql(name = "lastLogs")]
    async fn last_logs(&self, context: &Context, amount: Option<i32>) -> FieldResult<Vec<Log>> {
        let amount = amount.unwrap_or(20).max(0) as u64;
        let mut all_logs = Vec::new();

        for deployment in &self.deployments {
            let deployment_qid = deployment.deployment_qid().to_string();
            match load_deployment_logs(context, deployment_qid.clone(), amount).await {
                Ok(logs) => all_logs.extend(logs),
                Err(error) => {
                    tracing::warn!(
                        "Failed to fetch logs for deployment {deployment_qid}: {error}"
                    );
                }
            }
        }

        all_logs.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
        all_logs.truncate(amount as usize);
        Ok(all_logs)
    }
}

pub struct Deployment {
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

    fn commit(&self) -> String {
        self.deployment.deployment.to_string()
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
                juniper::FieldError::new("Internal server error", juniper::Value::Null)
            })?
            .map(|resource| resource.map(|resource| Resource { resource }))
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to load resources for deployment namespace {namespace} and owner {owner}: {e}"
                );
                juniper::FieldError::new("Internal server error", juniper::Value::Null)
            })
    }

    async fn artifacts(&self, context: &Context) -> FieldResult<Vec<Artifact>> {
        let namespace = self.deployment.deployment_qid().to_string();
        let artifacts = context.adb_client.list(&namespace).await.map_err(|error| {
            tracing::error!("Failed to list artifacts for deployment {namespace}: {error}");
            juniper::FieldError::new("Internal server error", juniper::Value::Null)
        })?;

        Ok(artifacts
            .into_iter()
            .map(|header| Artifact { header })
            .collect())
    }

    #[graphql(name = "lastLogs")]
    async fn last_logs(&self, context: &Context, amount: Option<i32>) -> FieldResult<Vec<Log>> {
        let amount = amount.unwrap_or(20).max(0) as u64;
        load_deployment_logs(context, self.deployment.deployment_qid().to_string(), amount)
            .await
            .map_err(|error| {
                tracing::error!(
                    "Failed to fetch deployment logs for {}: {}",
                    self.deployment.deployment_qid().to_string(),
                    error
                );
                juniper::FieldError::new("Internal server error", juniper::Value::Null)
            })
    }
}

pub struct Artifact {
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
                juniper::FieldError::new("Internal server error", juniper::Value::Null)
            })
    }
}

pub struct Resource {
    resource: rdb::Resource,
}

#[juniper::graphql_object(Context = Context)]
impl Resource {
    #[graphql(name = "type")]
    fn r#type(&self) -> &str {
        &self.resource.resource_type
    }

    fn id(&self) -> &str {
        &self.resource.id
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
                        juniper::FieldError::new("Internal server error", juniper::Value::Null)
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
                        juniper::FieldError::new("Internal server error", juniper::Value::Null)
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
                tracing::error!(
                    "Failed to list deployments for owner repository {repo_qid}: {e}"
                );
                juniper::FieldError::new("Internal server error", juniper::Value::Null)
            })?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to read deployments for owner repository {repo_qid}: {e}"
                );
                juniper::FieldError::new("Internal server error", juniper::Value::Null)
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
                .resource(dependency.ty.clone(), dependency.id.clone())
                .get()
                .await
                .map_err(|error| {
                    tracing::error!(
                        "Failed to load dependency {}/{} in namespace {}: {}",
                        dependency.ty,
                        dependency.id,
                        self.resource.namespace,
                        error
                    );
                    juniper::FieldError::new("Internal server error", juniper::Value::Null)
                })?;

            if let Some(resource) = resource {
                dependencies.push(Resource { resource });
            }
        }

        Ok(dependencies)
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
}

impl From<cdb::DeploymentState> for DeploymentState {
    fn from(state: cdb::DeploymentState) -> Self {
        match state {
            cdb::DeploymentState::Down => DeploymentState::Down,
            cdb::DeploymentState::Undesired => DeploymentState::Undesired,
            cdb::DeploymentState::Lingering => DeploymentState::Lingering,
            cdb::DeploymentState::Desired => DeploymentState::Desired,
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

pub struct Subscription;

type LogStream = Pin<Box<dyn Stream<Item = Log> + Send>>;

#[juniper::graphql_subscription(Context = Context)]
impl Subscription {
    async fn deployment_logs(
        context: &Context,
        deployment_id: String,
        initial_amount: Option<i32>,
    ) -> juniper::FieldResult<LogStream> {
        let (_, user) = context.check_auth().await?;

        let organization = deployment_organization(&deployment_id).ok_or_else(|| {
            juniper::FieldError::new("invalid deployment id", juniper::Value::Null)
        })?;

        if organization != user.username {
            tracing::warn!(
                "Rejected deployment logs subscription for deployment outside user organization: deployment={} user={}",
                deployment_id,
                user.username
            );
            return Err(juniper::FieldError::new(
                "deployment outside user organization",
                juniper::Value::Null,
            ));
        }

        let initial_amount = initial_amount.unwrap_or(1000).max(0) as u64;

        let consumer = ldb::ClientBuilder::new()
            .brokers(context.ldb_brokers.clone())
            .build_consumer()
            .await
            .map_err(|e| {
                tracing::error!("Failed to build ldb consumer for subscription: {}", e);
                juniper::FieldError::new("failed to tail logs", juniper::Value::Null)
            })?;

        let namespace = consumer.namespace(deployment_id).await.map_err(|e| {
            tracing::error!("Failed to prepare deployment logs subscription consumer: {e}");
            juniper::FieldError::new("failed to tail logs", juniper::Value::Null)
        })?;
        let mut inner = namespace
            .tail(ldb::TailConfig {
                follow: true,
                start_from: ldb::StartFrom::End(initial_amount),
            })
            .await
            .map_err(|e| {
                tracing::error!("Failed to tail deployment logs subscription: {e}");
                juniper::FieldError::new("failed to tail logs", juniper::Value::Null)
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
    ) -> juniper::FieldResult<LogStream> {
        let (_, user) = context.check_auth().await?;

        let env_qid: ids::EnvironmentQid = environment_qid.parse().map_err(|_| {
            juniper::FieldError::new("invalid environment QID", juniper::Value::Null)
        })?;

        let organization = env_qid.repo.org.to_string();
        if organization != user.username {
            tracing::warn!(
                "Rejected environment logs subscription for environment outside user organization: environment={} user={}",
                environment_qid,
                user.username
            );
            return Err(juniper::FieldError::new(
                "environment outside user organization",
                juniper::Value::Null,
            ));
        }

        let initial_amount = initial_amount.unwrap_or(1000).max(0) as u64;

        let consumer = ldb::ClientBuilder::new()
            .brokers(context.ldb_brokers.clone())
            .build_consumer()
            .await
            .map_err(|e| {
                tracing::error!("Failed to build ldb consumer for environment logs subscription: {e}");
                juniper::FieldError::new("failed to tail logs", juniper::Value::Null)
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
}

async fn load_deployment_logs(
    context: &Context,
    deployment_id: String,
    amount: u64,
) -> anyhow::Result<Vec<Log>> {
    let consumer = ldb::ClientBuilder::new()
        .brokers(context.ldb_brokers.clone())
        .build_consumer()
        .await?;
    let namespace = consumer.namespace(deployment_id).await?;
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

fn deployment_organization(deployment_qid: &str) -> Option<String> {
    let qid: ids::DeploymentQid = deployment_qid.parse().ok()?;
    Some(qid.repo_qid().org.to_string())
}

#[derive(Clone, Debug)]
#[graphql_scalar(with = json_scalar, parse_token(String), name = "JSON")]
pub struct JsonValue(pub serde_json::Value);

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

pub type Schema = RootNode<'static, Query, Mutation, Subscription>;

pub fn schema() -> Schema {
    Schema::new(Query, Mutation, Subscription)
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

    let app = Router::new()
        .route("/graphql", get(graphql_ws_handler).post(graphql_handler))
        .route("/graphiql", get(graphiql))
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
    let auth_header = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|h| h.as_bytes().strip_prefix(b"Bearer "))
        .and_then(|v| String::from_utf8(v.to_vec()).ok());

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
    let auth_header = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|h| h.as_bytes().strip_prefix(b"Bearer "))
        .and_then(|v| String::from_utf8(v.to_vec()).ok());

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
        .on_upgrade(move |socket| graphql_ws_connection(socket, schema, context))
}

async fn graphql_ws_connection(mut socket: WebSocket, schema: Arc<Schema>, context: Context) {
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
        .send(WsMessage::Text(value.to_string().into()))
        .await
        .is_ok()
}

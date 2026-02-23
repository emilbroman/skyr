use std::sync::Arc;

mod challenge;

use axum::{
    Json, Router,
    extract::Extension,
    response::Html,
    routing::{get, post},
};
use chrono::Utc;
use clap::Parser;
use futures_util::{StreamExt, TryStreamExt};
use juniper::{EmptySubscription, FieldResult, RootNode};

pub struct Context {
    udb_client: udb::Client,
    cdb_client: cdb::Client,
    rdb_client: rdb::Client,
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
        let _ = (&context.cdb_client, &context.rdb_client);
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

        let name = cdb::RepositoryName {
            organization,
            repository,
        };

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
        self.repository.name.repository.clone()
    }

    async fn deployments(&self, context: &Context) -> FieldResult<Vec<Deployment>> {
        context
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
            .map(|d| d.map(|deployment| Deployment { deployment }))
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to read deployments for {}: {}",
                    self.repository.name,
                    e
                );
                juniper::FieldError::new("Internal server error", juniper::Value::Null)
            })
    }
}

pub struct Deployment {
    deployment: cdb::Deployment,
}

#[juniper::graphql_object(Context = Context)]
impl Deployment {
    fn state(&self) -> DeploymentState {
        self.deployment.state.into()
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

pub type Schema = RootNode<'static, Query, Mutation, EmptySubscription<Context>>;

pub fn schema() -> Schema {
    Schema::new(Query, Mutation, EmptySubscription::new())
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
    #[arg(long)]
    challenge_salt: Option<String>,
    #[arg(long)]
    write_schema: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
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
    let challenger = Arc::new(challenge::Challenger::new(challenge_salt.into_bytes()));

    let schema = Arc::new(schema());

    let app = Router::new()
        .route("/graphql", post(graphql_handler))
        .route("/graphiql", get(graphiql))
        .layer(Extension(schema))
        .layer(Extension(challenger))
        .layer(Extension(cdb_client))
        .layer(Extension(rdb_client))
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
    Extension(mut udb_client): Extension<udb::Client>,
    headers: http::header::HeaderMap,
    Json(request): Json<juniper::http::GraphQLRequest>,
) -> Json<juniper::http::GraphQLResponse> {
    let auth_header = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|h| h.as_bytes().strip_prefix(b"Bearer "))
        .and_then(|v| String::from_utf8(v.to_vec()).ok());

    if let Some(token) = auth_header {
        match udb_client.lookup_token(token).await {
            Err(udb::LookupTokenError::InvalidToken) => {
                return Json(juniper::http::GraphQLResponse::error(
                    "Invalid token".into(),
                ));
            }
            Err(e) => {
                tracing::error!("Failed to lookup token: {}", e);
                return Json(juniper::http::GraphQLResponse::error(
                    "Internal server error".into(),
                ));
            }
            Ok(user) => {
                let ctx = Context {
                    udb_client,
                    cdb_client,
                    rdb_client,
                    challenger,
                    user: Some(user),
                };
                return Json(request.execute(&schema, &ctx).await);
            }
        }
    }

    let ctx = Context {
        udb_client,
        cdb_client,
        rdb_client,
        challenger,
        user: None,
    };
    Json(request.execute(&schema, &ctx).await)
}

async fn graphiql() -> Html<String> {
    Html(juniper::http::graphiql::graphiql_source("/graphql", None))
}

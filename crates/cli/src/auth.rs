use anyhow::{Context, anyhow};
use clap::{Args, Subcommand};
use graphql_client::GraphQLQuery;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use zeroize::Zeroizing;

use crate::context::Context as CliContext;
use crate::output::OutputFormat;

/// Custom scalar required by `graphql_client` derive for the `JSON` scalar in the schema.
#[allow(clippy::upper_case_acronyms)]
pub(crate) type JSON = serde_json::Value;

const SIGNATURE_NAMESPACE: &str = "skyr-auth-challenge";

/// Expected minimum length for a valid token (8 hex chars + separator + payload).
const MIN_TOKEN_LENGTH: usize = 10;

/// Maximum token length to prevent abuse when constructing HTTP headers.
const MAX_TOKEN_LENGTH: usize = 4096;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "../api/schema.graphql",
    query_path = "src/graphql/auth_challenge.graphql",
    response_derives = "Debug, Serialize, Deserialize"
)]
struct AuthChallenge;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "../api/schema.graphql",
    query_path = "src/graphql/signin.graphql",
    response_derives = "Debug, Serialize, Deserialize"
)]
struct Signin;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct UserConfig {
    pub(crate) username: String,
    pub(crate) key: String,
}

pub(crate) async fn acquire_token(
    client: &reqwest::Client,
    api_url: &str,
) -> anyhow::Result<String> {
    if let Ok(token) = read_token().await
        && !is_expired_token(&token)?
    {
        return Ok(token);
    }

    let user = read_user_config().await?;
    let endpoint = graphql_endpoint(api_url);
    let token = signin_with_key(client, &endpoint, &user.username, Path::new(&user.key)).await?;
    write_token(&token).await?;
    Ok(token)
}

pub(crate) async fn signin_with_key(
    client: &reqwest::Client,
    endpoint: &str,
    username: &str,
    key_path: &Path,
) -> anyhow::Result<String> {
    let proof = build_auth_proof(client, endpoint, username, key_path).await?;
    let data = graphql_query_unauth::<Signin>(
        client,
        endpoint,
        signin::Variables {
            username: username.to_owned(),
            proof: serde_json::Value::String(proof),
        },
        "signin",
    )
    .await?;
    Ok(data.signin.token)
}

pub(crate) async fn build_auth_proof(
    client: &reqwest::Client,
    endpoint: &str,
    username: &str,
    key_path: &Path,
) -> anyhow::Result<String> {
    let key = Zeroizing::new(
        tokio::fs::read_to_string(key_path)
            .await
            .with_context(|| format!("failed to read private key at {}", key_path.display()))?,
    );
    let private_key = russh::keys::ssh_key::PrivateKey::from_openssh(key.as_str())
        .context("failed to parse private key")?;

    let challenge = query_auth_challenge(client, endpoint, username).await?;
    let signature = private_key
        .sign(
            SIGNATURE_NAMESPACE,
            russh::keys::ssh_key::HashAlg::default(),
            challenge.as_bytes(),
        )
        .context("failed to sign auth challenge")?
        .to_string();

    Ok(signature)
}

async fn query_auth_challenge(
    client: &reqwest::Client,
    endpoint: &str,
    username: &str,
) -> anyhow::Result<String> {
    let data = graphql_query_unauth::<AuthChallenge>(
        client,
        endpoint,
        auth_challenge::Variables {
            username: username.to_owned(),
        },
        "auth challenge",
    )
    .await?;
    Ok(data.auth_challenge.challenge)
}

pub(crate) async fn persist_auth_state(
    username: &str,
    key_path: &Path,
    token: &str,
) -> anyhow::Result<()> {
    let token_path = token_cache_path()?;
    let user_config_path = user_config_path()?;

    if let Some(parent) = token_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    if let Some(parent) = user_config_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    write_file_restricted(&token_path, token.as_bytes()).await?;

    let user_config = UserConfig {
        username: username.to_owned(),
        key: key_path.display().to_string(),
    };
    let user_config_json = serde_json::to_string_pretty(&user_config)?;
    write_file_restricted(&user_config_path, user_config_json.as_bytes()).await?;

    Ok(())
}

/// Write `data` to `path` with mode 0o600 (owner read/write only).
async fn write_file_restricted(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    tokio::fs::write(path, data)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .await
            .with_context(|| format!("failed to set permissions on {}", path.display()))?;
    }

    Ok(())
}

pub(crate) fn graphql_response_data<T>(
    response: graphql_client::Response<T>,
    operation: &str,
) -> anyhow::Result<T> {
    if let Some(errors) = response.errors {
        let messages: Vec<String> = errors
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let mut msg = format!("  {}. {}", i + 1, e.message);
                if let Some(ref locations) = e.locations
                    && !locations.is_empty()
                {
                    let locs: Vec<String> = locations
                        .iter()
                        .map(|loc| format!("line {}:{}", loc.line, loc.column))
                        .collect();
                    msg.push_str(&format!(" (at {})", locs.join(", ")));
                }
                if let Some(ref path) = e.path
                    && !path.is_empty()
                {
                    let path_strs: Vec<String> = path.iter().map(|p| format!("{p}")).collect();
                    msg.push_str(&format!(" [path: {}]", path_strs.join(".")));
                }
                msg
            })
            .collect();
        return Err(anyhow!(
            "{operation} failed with {} error(s):\n{}",
            messages.len(),
            messages.join("\n")
        ));
    }
    response
        .data
        .ok_or_else(|| anyhow!("{operation} response did not include data"))
}

/// Execute an authenticated GraphQL query/mutation and return the response data.
pub(crate) async fn graphql_query<Q: GraphQLQuery>(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
    variables: Q::Variables,
    operation: &str,
) -> anyhow::Result<Q::ResponseData>
where
    Q::ResponseData: serde::de::DeserializeOwned,
{
    let body = Q::build_query(variables);
    let response = client
        .post(endpoint)
        .header(reqwest::header::AUTHORIZATION, bearer_header_value(token)?)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("failed to send {operation}"))?;
    let response: graphql_client::Response<Q::ResponseData> = response
        .json()
        .await
        .with_context(|| format!("failed to decode {operation} response"))?;
    graphql_response_data(response, operation)
}

/// Execute an unauthenticated GraphQL query/mutation and return the response data.
pub(crate) async fn graphql_query_unauth<Q: GraphQLQuery>(
    client: &reqwest::Client,
    endpoint: &str,
    variables: Q::Variables,
    operation: &str,
) -> anyhow::Result<Q::ResponseData>
where
    Q::ResponseData: serde::de::DeserializeOwned,
{
    let body = Q::build_query(variables);
    let response = client
        .post(endpoint)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("failed to send {operation}"))?;
    let response: graphql_client::Response<Q::ResponseData> = response
        .json()
        .await
        .with_context(|| format!("failed to decode {operation} response"))?;
    graphql_response_data(response, operation)
}

/// Construct an `Authorization: Bearer {token}` header value, validating the
/// token length to prevent header injection or unreasonably large values.
pub(crate) fn bearer_header_value(token: &str) -> anyhow::Result<reqwest::header::HeaderValue> {
    if token.len() < MIN_TOKEN_LENGTH {
        return Err(anyhow!("token is too short to be valid"));
    }
    if token.len() > MAX_TOKEN_LENGTH {
        return Err(anyhow!("token exceeds maximum allowed length"));
    }
    reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
        .context("token contains invalid header characters")
}

pub(crate) fn graphql_endpoint(api_url: &str) -> String {
    let base = if api_url.starts_with("http://") || api_url.starts_with("https://") {
        api_url.to_string()
    } else {
        format!("http://{api_url}")
    };

    if base.ends_with("/graphql") {
        base
    } else {
        format!("{}/graphql", base.trim_end_matches('/'))
    }
}

pub(crate) fn expand_tilde(path: &str) -> anyhow::Result<PathBuf> {
    if path == "~" {
        return home_dir();
    }

    if let Some(suffix) = path.strip_prefix("~/") {
        return home_dir().map(|home| home.join(suffix));
    }

    Ok(Path::new(path).to_path_buf())
}

async fn read_token() -> anyhow::Result<String> {
    let token_path = token_cache_path()?;
    Ok(tokio::fs::read_to_string(&token_path)
        .await
        .with_context(|| format!("failed to read {}", token_path.display()))?
        .trim()
        .to_owned())
}

async fn write_token(token: &str) -> anyhow::Result<()> {
    let token_path = token_cache_path()?;
    if let Some(parent) = token_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    write_file_restricted(&token_path, token.as_bytes()).await
}

pub(crate) async fn read_user_config() -> anyhow::Result<UserConfig> {
    let user_config_path = user_config_path()?;
    let contents = tokio::fs::read_to_string(&user_config_path)
        .await
        .with_context(|| format!("failed to read {}", user_config_path.display()))?;
    serde_json::from_str::<UserConfig>(&contents)
        .with_context(|| format!("failed to parse {}", user_config_path.display()))
}

/// Parse the expiry prefix from a token.
///
/// Token format: `<8 hex digits for expiry>.<payload>`
struct TokenExpiry {
    expiry_epoch: u32,
}

impl TokenExpiry {
    fn parse(token: &str) -> anyhow::Result<Self> {
        if token.len() < MIN_TOKEN_LENGTH {
            return Err(anyhow!("token is too short to contain expiry prefix"));
        }
        let (expiry_hex, rest) = token.split_at(8);
        if !rest.starts_with('.') {
            return Err(anyhow!(
                "token has invalid separator (expected '.' at position 8)"
            ));
        }
        let expiry_epoch =
            u32::from_str_radix(expiry_hex, 16).context("token expiry is not valid hex")?;
        Ok(Self { expiry_epoch })
    }

    fn is_expired(&self) -> anyhow::Result<bool> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock before unix epoch")?
            .as_secs();
        Ok(now >= u64::from(self.expiry_epoch))
    }
}

fn is_expired_token(token: &str) -> anyhow::Result<bool> {
    TokenExpiry::parse(token)?.is_expired()
}

fn token_cache_path() -> anyhow::Result<PathBuf> {
    Ok(home_dir()?.join(".cache").join("skyr_token"))
}

fn user_config_path() -> anyhow::Result<PathBuf> {
    Ok(home_dir()?.join(".config").join("skyr").join("user.json"))
}

pub(crate) fn home_dir() -> anyhow::Result<PathBuf> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")
}

// --- subcommand entry points -------------------------------------------------

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "../api/schema.graphql",
    query_path = "src/graphql/signup.graphql",
    response_derives = "Debug, serde::Serialize"
)]
struct Signup;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "../api/schema.graphql",
    query_path = "src/graphql/me.graphql",
    response_derives = "Debug, serde::Serialize"
)]
struct Me;

#[derive(Args, Debug)]
pub struct AuthArgs {
    #[command(subcommand)]
    command: AuthCommand,
}

#[derive(Subcommand, Debug)]
enum AuthCommand {
    /// Sign in to Skyr using an SSH private key.
    Signin {
        #[arg(long)]
        username: String,
        #[arg(long, default_value = "~/.ssh/id_ed25519")]
        key: String,
    },
    /// Create a new Skyr account using an SSH private key.
    Signup {
        #[arg(long)]
        username: String,
        #[arg(long)]
        email: String,
        #[arg(long)]
        fullname: Option<String>,
        #[arg(long, default_value = "~/.ssh/id_ed25519")]
        key: String,
    },
    /// Forget the cached token and stored user config.
    Signout,
    /// Show the currently signed-in user.
    Whoami,
}

pub async fn run_auth(args: AuthArgs, ctx: &CliContext) -> anyhow::Result<()> {
    match args.command {
        AuthCommand::Signin { username, key } => run_signin(ctx, &username, &key).await,
        AuthCommand::Signup {
            username,
            email,
            fullname,
            key,
        } => run_signup(ctx, &username, &email, fullname.as_deref(), &key).await,
        AuthCommand::Signout => run_signout(ctx.format).await,
        AuthCommand::Whoami => run_whoami(ctx).await,
    }
}

async fn run_signin(ctx: &CliContext, username: &str, key: &str) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let endpoint = graphql_endpoint(ctx.api_url());
    let key_path = expand_tilde(key)?;

    let token = signin_with_key(&client, &endpoint, username, &key_path).await?;
    persist_auth_state(username, &key_path, &token).await?;

    #[derive(Serialize)]
    struct SigninOutput<'a> {
        username: &'a str,
    }
    let output = SigninOutput { username };

    match ctx.format {
        OutputFormat::Json => crate::output::print_json(&output)?,
        OutputFormat::Text => {
            let mut table = crate::output::table("{:<}  {:<}");
            table.add_row(crate::output::row(vec!["FIELD".into(), "VALUE".into()]));
            table.add_row(crate::output::row(vec![
                "username".into(),
                output.username.to_owned(),
            ]));
            println!("Token saved to credentials file.");
            print!("{table}");
        }
    }
    Ok(())
}

async fn run_signup(
    ctx: &CliContext,
    username: &str,
    email: &str,
    fullname: Option<&str>,
    key: &str,
) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let endpoint = graphql_endpoint(ctx.api_url());
    let key_path = expand_tilde(key)?;

    let proof = build_auth_proof(&client, &endpoint, username, &key_path).await?;

    let data = graphql_query_unauth::<Signup>(
        &client,
        &endpoint,
        signup::Variables {
            username: username.to_owned(),
            email: email.to_owned(),
            proof: serde_json::Value::String(proof),
            fullname: fullname.map(str::to_owned),
        },
        "signup",
    )
    .await?;
    persist_auth_state(&data.signup.user.username, &key_path, &data.signup.token).await?;

    #[derive(Serialize)]
    struct SignupUserOutput {
        username: String,
        email: String,
        fullname: Option<String>,
    }
    #[derive(Serialize)]
    struct SignupOutput {
        user: SignupUserOutput,
    }

    let output = SignupOutput {
        user: SignupUserOutput {
            username: data.signup.user.username,
            email: data.signup.user.email,
            fullname: data.signup.user.fullname,
        },
    };

    match ctx.format {
        OutputFormat::Json => crate::output::print_json(&output)?,
        OutputFormat::Text => {
            let mut table = crate::output::table("{:<}  {:<}");
            table.add_row(crate::output::row(vec!["FIELD".into(), "VALUE".into()]));
            table.add_row(crate::output::row(vec![
                "username".into(),
                output.user.username,
            ]));
            table.add_row(crate::output::row(vec!["email".into(), output.user.email]));
            table.add_row(crate::output::row(vec![
                "fullname".into(),
                output.user.fullname.unwrap_or_default(),
            ]));
            println!("Token saved to credentials file.");
            print!("{table}");
        }
    }
    Ok(())
}

async fn run_signout(format: OutputFormat) -> anyhow::Result<()> {
    let token_path = token_cache_path()?;
    let user_config_path = user_config_path()?;
    let mut removed: Vec<String> = Vec::new();

    for path in [&token_path, &user_config_path] {
        match tokio::fs::remove_file(path).await {
            Ok(()) => removed.push(path.display().to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(anyhow!("failed to remove {}: {e}", path.display()));
            }
        }
    }

    #[derive(Serialize)]
    struct SignoutOutput {
        removed: Vec<String>,
    }
    let output = SignoutOutput { removed };

    match format {
        OutputFormat::Json => crate::output::print_json(&output)?,
        OutputFormat::Text => {
            if output.removed.is_empty() {
                println!("Already signed out.");
            } else {
                println!("Signed out. Removed:");
                for path in &output.removed {
                    println!("  {path}");
                }
            }
        }
    }
    Ok(())
}

async fn run_whoami(ctx: &CliContext) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let token = acquire_token(&client, ctx.api_url()).await?;
    let endpoint = graphql_endpoint(ctx.api_url());

    let data = graphql_query::<Me>(&client, &endpoint, &token, me::Variables {}, "whoami").await?;

    #[derive(Serialize)]
    struct WhoamiOutput {
        username: String,
        email: String,
        fullname: Option<String>,
    }

    let output = WhoamiOutput {
        username: data.me.username,
        email: data.me.email,
        fullname: data.me.fullname,
    };

    match ctx.format {
        OutputFormat::Json => crate::output::print_json(&output)?,
        OutputFormat::Text => {
            let mut table = crate::output::table("{:<}  {:<}");
            table.add_row(crate::output::row(vec!["FIELD".into(), "VALUE".into()]));
            table.add_row(crate::output::row(vec!["username".into(), output.username]));
            table.add_row(crate::output::row(vec!["email".into(), output.email]));
            table.add_row(crate::output::row(vec![
                "fullname".into(),
                output.fullname.unwrap_or_default(),
            ]));
            print!("{table}");
        }
    }
    Ok(())
}

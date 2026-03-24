use anyhow::{Context, anyhow};
use graphql_client::GraphQLQuery;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use zeroize::Zeroizing;

/// Custom scalar required by `graphql_client` derive for the `JSON` scalar in the schema.
#[allow(clippy::upper_case_acronyms)]
type JSON = serde_json::Value;

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
struct UserConfig {
    username: String,
    key: String,
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
    let body = Signin::build_query(signin::Variables {
        username: username.to_owned(),
        proof: serde_json::Value::String(proof),
    });

    let response = client
        .post(endpoint)
        .json(&body)
        .send()
        .await
        .context("failed to send signin mutation")?;
    let response: graphql_client::Response<signin::ResponseData> = response
        .json()
        .await
        .context("failed to decode signin response")?;
    let data = graphql_response_data(response, "signin")?;
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
    let body = AuthChallenge::build_query(auth_challenge::Variables {
        username: username.to_owned(),
    });

    let response = client
        .post(endpoint)
        .json(&body)
        .send()
        .await
        .context("failed to fetch auth challenge")?;

    let response: graphql_client::Response<auth_challenge::ResponseData> = response
        .json()
        .await
        .context("failed to decode auth challenge response")?;
    let data = graphql_response_data(response, "auth challenge")?;
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

async fn read_user_config() -> anyhow::Result<UserConfig> {
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

fn home_dir() -> anyhow::Result<PathBuf> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")
}

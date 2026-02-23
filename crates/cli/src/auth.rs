use anyhow::{Context, anyhow};
use graphql_client::GraphQLQuery;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const SIGNATURE_NAMESPACE: &str = "skyr-auth-challenge";

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
pub struct UserConfig {
    pub username: String,
    pub key: String,
}

#[derive(Debug, Clone)]
pub struct AuthProof {
    pub pubkey: String,
    pub signature: String,
}

pub async fn acquire_token(client: &reqwest::Client, api_url: &str) -> anyhow::Result<String> {
    if let Ok(token) = read_token().await {
        if !is_expired_token(&token)? {
            return Ok(token);
        }
    }

    let user = read_user_config().await?;
    let endpoint = graphql_endpoint(api_url);
    let token = signin_with_key(client, &endpoint, &user.username, Path::new(&user.key)).await?;
    write_token(&token).await?;
    Ok(token)
}

pub async fn signin_with_key(
    client: &reqwest::Client,
    endpoint: &str,
    username: &str,
    key_path: &Path,
) -> anyhow::Result<String> {
    let proof = build_auth_proof(client, endpoint, username, key_path).await?;
    let body = Signin::build_query(signin::Variables {
        username: username.to_owned(),
        signature: proof.signature,
        pubkey: proof.pubkey,
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

    if let Some(errors) = response.errors {
        return Err(anyhow!(
            "signin failed: {}",
            errors
                .iter()
                .map(|e| e.message.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }

    let data = response
        .data
        .context("signin response did not include data")?;
    Ok(data.signin.token)
}

pub async fn build_auth_proof(
    client: &reqwest::Client,
    endpoint: &str,
    username: &str,
    key_path: &Path,
) -> anyhow::Result<AuthProof> {
    let key = tokio::fs::read_to_string(key_path)
        .await
        .with_context(|| format!("failed to read private key at {}", key_path.display()))?;
    let private_key = russh::keys::ssh_key::PrivateKey::from_openssh(&key)
        .context("failed to parse private key")?;
    let public_key = private_key
        .public_key()
        .to_openssh()
        .context("failed to encode derived public key in OpenSSH format")?;

    let challenge = query_auth_challenge(client, endpoint, username).await?;
    let signature = private_key
        .sign(
            SIGNATURE_NAMESPACE,
            russh::keys::ssh_key::HashAlg::default(),
            challenge.as_bytes(),
        )
        .context("failed to sign auth challenge")?
        .to_string();

    Ok(AuthProof {
        pubkey: public_key,
        signature,
    })
}

pub async fn query_auth_challenge(
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

    if let Some(errors) = response.errors {
        return Err(anyhow!(
            "auth challenge failed: {}",
            errors
                .iter()
                .map(|e| e.message.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }

    let data = response
        .data
        .context("auth challenge response did not include data")?;
    Ok(data.auth_challenge)
}

pub async fn persist_auth_state(
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

    tokio::fs::write(&token_path, token)
        .await
        .with_context(|| format!("failed to write {}", token_path.display()))?;

    let user_config = UserConfig {
        username: username.to_owned(),
        key: key_path.display().to_string(),
    };
    let user_config_json = serde_json::to_string_pretty(&user_config)?;
    tokio::fs::write(&user_config_path, user_config_json)
        .await
        .with_context(|| format!("failed to write {}", user_config_path.display()))?;

    Ok(())
}

pub fn graphql_endpoint(api_url: &str) -> String {
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

pub fn expand_tilde(path: &str) -> anyhow::Result<PathBuf> {
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
    tokio::fs::write(&token_path, token)
        .await
        .with_context(|| format!("failed to write {}", token_path.display()))?;
    Ok(())
}

async fn read_user_config() -> anyhow::Result<UserConfig> {
    let user_config_path = user_config_path()?;
    let contents = tokio::fs::read_to_string(&user_config_path)
        .await
        .with_context(|| format!("failed to read {}", user_config_path.display()))?;
    serde_json::from_str::<UserConfig>(&contents)
        .with_context(|| format!("failed to parse {}", user_config_path.display()))
}

fn is_expired_token(token: &str) -> anyhow::Result<bool> {
    let expiry_hex = token
        .get(0..8)
        .ok_or_else(|| anyhow!("token is missing expiry prefix"))?;
    let separator = token
        .as_bytes()
        .get(8)
        .copied()
        .ok_or_else(|| anyhow!("token is missing separator"))?;
    if separator != b'.' {
        return Err(anyhow!("token has invalid separator"));
    }

    let expiry = u32::from_str_radix(expiry_hex, 16).context("token expiry is not valid hex")?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock before unix epoch")?
        .as_secs();
    Ok(now >= u64::from(expiry))
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

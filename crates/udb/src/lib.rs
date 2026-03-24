use base64::Engine;
use rand::{Rng, SeedableRng};
use redis::{AsyncCommands, Client as RedisClient};
use sha2::Digest;
use ssh_key::PublicKey;
use std::time::SystemTime;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConnectError {
    #[error("failed to create redis client: {0}")]
    RedisClient(#[from] redis::RedisError),

    #[error("failed to connect to redis server: {0}")]
    RedisConnection(#[source] redis::RedisError),
}

#[derive(Default)]
pub struct ClientBuilder {
    known_nodes: Vec<String>,
}

impl ClientBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn known_node(mut self, hostname: impl AsRef<str>) -> Self {
        self.known_nodes.push(hostname.as_ref().to_owned());
        self
    }

    pub async fn build(&self) -> Result<Client, ConnectError> {
        let node = self
            .known_nodes
            .first()
            .cloned()
            .unwrap_or_else(|| "127.0.0.1".to_owned());
        let url = format!("redis://{node}/");

        let redis_client = RedisClient::open(url)?;
        let conn = redis_client
            .get_multiplexed_async_connection()
            .await
            .map_err(ConnectError::RedisConnection)?;

        Ok(Client {
            conn,
            rng: rand::rngs::StdRng::from_os_rng(),
        })
    }
}

#[derive(Error, Debug)]
pub enum RegisterUserError {
    #[error("failed to execute query: {0}")]
    Redis(#[from] redis::RedisError),

    #[error("username already taken")]
    UsernameTaken,

    #[error("invalid username: {0}")]
    InvalidUsername(String),
}

#[derive(Error, Debug)]
pub enum UserQueryError {
    #[error("failed to execute query: {0}")]
    Redis(#[from] redis::RedisError),

    #[error("user not found")]
    NotFound,

    #[error("system clock error: {0}")]
    Clock(#[from] std::time::SystemTimeError),

    #[error("token expiry cannot be represented as u32 epoch seconds")]
    InvalidTokenExpiry,

    #[error("invalid SSH public key: {0}")]
    InvalidPublicKey(#[from] ssh_key::Error),

    #[error("invalid username: {0}")]
    InvalidUsername(String),

    #[error("corrupted data: {0}")]
    DataCorruption(String),
}

#[derive(Error, Debug)]
pub enum LookupTokenError {
    #[error("failed to execute query: {0}")]
    Redis(#[from] redis::RedisError),

    #[error("invalid token")]
    InvalidToken,

    #[error("token has expired")]
    Expired,

    #[error("system clock error: {0}")]
    Clock(#[from] std::time::SystemTimeError),
}

#[derive(Error, Debug)]
pub enum CoseKeyError {
    #[error("invalid CBOR encoding: {0}")]
    Cbor(String),
    #[error("unsupported COSE algorithm: {0}")]
    UnsupportedAlgorithm(i64),
    #[error("missing COSE key parameter: {0}")]
    MissingParameter(&'static str),
    #[error("invalid key data: {0}")]
    InvalidKeyData(String),
}

#[derive(Debug, Clone)]
pub struct Credential {
    pub fingerprint: String,
    pub public_key: String,
    pub credential_id: Option<String>,
    pub sign_count: u32,
}

#[derive(Debug, Clone)]
pub struct User {
    pub username: String,
    pub email: String,
    pub fullname: Option<String>,
}

#[derive(Clone)]
pub struct Client {
    conn: redis::aio::MultiplexedConnection,
    rng: rand::rngs::StdRng,
}

const TOKEN_TTL_SECONDS: u64 = 86400;

fn user_key(username: &str) -> String {
    format!("u:{username}")
}

fn pubkey_key(username: &str) -> String {
    format!("p:{username}")
}

fn token_key(token: &str) -> String {
    format!("t:{token}")
}

fn credential_key(username: &str, fingerprint: &str) -> String {
    format!("c:{username}:{fingerprint}")
}

/// Validates that a username is safe for use in Redis key construction.
/// Returns the validation error message if invalid, or Ok(()) if valid.
fn validate_username(username: &str) -> Result<(), String> {
    if username.is_empty() {
        return Err("username must not be empty".into());
    }
    if username.contains(':') {
        return Err("username must not contain ':'".into());
    }
    Ok(())
}

impl Client {
    pub fn user(&self, username: impl Into<String>) -> UserClient {
        UserClient {
            client: self.clone(),
            username: username.into(),
        }
    }

    pub async fn lookup_token(
        &mut self,
        token: impl Into<String>,
    ) -> Result<UserClient, LookupTokenError> {
        let token = token.into();

        // Validate embedded expiry before hitting Redis (defense-in-depth
        // beyond Redis TTL). Token format: "{expiry_hex}.{raw_token}".
        if let Some(dot_pos) = token.find('.') {
            let expiry_hex = &token[..dot_pos];
            if let Ok(expiry_bytes) = hex::decode(expiry_hex)
                && expiry_bytes.len() == 4
            {
                let expiry = u32::from_be_bytes([
                    expiry_bytes[0],
                    expiry_bytes[1],
                    expiry_bytes[2],
                    expiry_bytes[3],
                ]);
                let now = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)?
                    .as_secs();
                if u64::from(expiry) < now {
                    return Err(LookupTokenError::Expired);
                }
            }
        }

        let username: Option<String> = self.conn.get(token_key(&token)).await?;

        match username {
            Some(username) => Ok(self.user(username)),
            None => Err(LookupTokenError::InvalidToken),
        }
    }
}

#[derive(Clone)]
pub struct UserClient {
    client: Client,
    username: String,
}

impl UserClient {
    pub fn pubkeys(&self) -> PubkeysClient {
        PubkeysClient { user: self.clone() }
    }

    pub fn tokens(&self) -> TokensClient {
        TokensClient { user: self.clone() }
    }

    pub async fn register(
        &mut self,
        email: impl Into<String>,
        fullname: Option<String>,
    ) -> Result<User, RegisterUserError> {
        validate_username(&self.username).map_err(RegisterUserError::InvalidUsername)?;
        let email = email.into();

        let result: i32 = self
            .client
            .conn
            .hset_nx(user_key(&self.username), "email", &email)
            .await?;

        if result == 0 {
            return Err(RegisterUserError::UsernameTaken);
        }

        if let Some(ref fullname) = fullname {
            let _: () = self
                .client
                .conn
                .hset(user_key(&self.username), "fullname", fullname)
                .await?;
        }

        Ok(User {
            username: self.username.clone(),
            email,
            fullname,
        })
    }

    pub async fn set_fullname(
        &mut self,
        fullname: impl Into<String>,
    ) -> Result<(), UserQueryError> {
        let fullname = fullname.into();
        let key = user_key(&self.username);

        if fullname.is_empty() {
            let _: () = self.client.conn.hdel(&key, "fullname").await?;
        } else {
            let _: () = self.client.conn.hset(&key, "fullname", &fullname).await?;
        }

        Ok(())
    }

    pub async fn get(&mut self) -> Result<User, UserQueryError> {
        let (email, fullname): (Option<String>, Option<String>) = self
            .client
            .conn
            .hmget(user_key(&self.username), &["email", "fullname"])
            .await?;

        let Some(email) = email else {
            return Err(UserQueryError::NotFound);
        };

        Ok(User {
            username: self.username.clone(),
            email,
            fullname: fullname.filter(|s: &String| !s.is_empty()),
        })
    }
}

#[derive(Clone)]
pub struct TokensClient {
    user: UserClient,
}

impl TokensClient {
    pub async fn issue(&mut self) -> Result<String, UserQueryError> {
        let mut raw_token = String::new();

        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode_string(self.user.client.rng.random::<[u8; 32]>(), &mut raw_token);

        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)?
            .as_secs();
        let expiry = now + TOKEN_TTL_SECONDS;
        let expiry_u32 = u32::try_from(expiry).map_err(|_| UserQueryError::InvalidTokenExpiry)?;
        let expiry_hex = hex::encode(expiry_u32.to_be_bytes());
        let token = format!("{expiry_hex}.{raw_token}");

        let _: () = self
            .user
            .client
            .conn
            .set_ex(token_key(&token), &self.user.username, TOKEN_TTL_SECONDS)
            .await?;

        Ok(token)
    }

    pub async fn revoke(&mut self, token: impl Into<String>) -> Result<(), LookupTokenError> {
        let token = token.into();

        // Atomically delete the key only if its value matches the expected
        // username. This replaces the non-standard DELEX command with a
        // standard Lua script that works on all Redis versions.
        let script = redis::Script::new(
            r#"
            if redis.call('GET', KEYS[1]) == ARGV[1] then
                return redis.call('DEL', KEYS[1])
            else
                return 0
            end
            "#,
        );
        let deleted: i32 = script
            .key(token_key(&token))
            .arg(&self.user.username)
            .invoke_async(&mut self.user.client.conn)
            .await?;

        if deleted == 0 {
            return Err(LookupTokenError::InvalidToken);
        }

        Ok(())
    }
}

#[derive(Clone)]
pub struct PubkeysClient {
    user: UserClient,
}

impl PubkeysClient {
    pub async fn list(&mut self) -> Result<Vec<String>, UserQueryError> {
        let members: Vec<String> = self
            .user
            .client
            .conn
            .smembers(pubkey_key(&self.user.username))
            .await?;

        Ok(members)
    }

    pub async fn add(&mut self, pubkey: impl Into<String>) -> Result<(), UserQueryError> {
        let pubkey = pubkey.into();

        let _: () = self
            .user
            .client
            .conn
            .sadd(pubkey_key(&self.user.username), &pubkey)
            .await?;

        Ok(())
    }

    pub async fn contains(&mut self, pubkey: impl Into<String>) -> Result<bool, UserQueryError> {
        let pubkey = pubkey.into();

        let contains: bool = self
            .user
            .client
            .conn
            .sismember(pubkey_key(&self.user.username), &pubkey)
            .await?;

        Ok(contains)
    }

    pub async fn remove(&mut self, pubkey: impl Into<String>) -> Result<(), UserQueryError> {
        let pubkey = pubkey.into();

        let _: () = self
            .user
            .client
            .conn
            .srem(pubkey_key(&self.user.username), &pubkey)
            .await?;

        let _: () = self
            .user
            .client
            .conn
            .del(credential_key(&self.user.username, &pubkey))
            .await?;

        Ok(())
    }

    pub async fn add_credential(
        &mut self,
        public_key: &str,
        credential_id: Option<&str>,
        sign_count: u32,
    ) -> Result<Credential, UserQueryError> {
        let parsed = PublicKey::from_openssh(public_key)?;
        let fingerprint = parsed.fingerprint(ssh_key::HashAlg::Sha256).to_string();

        let _: () = self
            .user
            .client
            .conn
            .sadd(pubkey_key(&self.user.username), &fingerprint)
            .await?;

        let cred_key = credential_key(&self.user.username, &fingerprint);
        let cred_id_str = credential_id.unwrap_or("");

        redis::pipe()
            .hset(&cred_key, "public_key", public_key)
            .hset(&cred_key, "credential_id", cred_id_str)
            .hset(&cred_key, "sign_count", sign_count.to_string())
            .exec_async(&mut self.user.client.conn)
            .await?;

        Ok(Credential {
            fingerprint,
            public_key: public_key.to_owned(),
            credential_id: credential_id.map(|s| s.to_owned()),
            sign_count,
        })
    }

    pub async fn get_credential(
        &mut self,
        fingerprint: &str,
    ) -> Result<Credential, UserQueryError> {
        let cred_key = credential_key(&self.user.username, fingerprint);
        let (public_key, credential_id, sign_count_str): (
            Option<String>,
            Option<String>,
            Option<String>,
        ) = self
            .user
            .client
            .conn
            .hget(&cred_key, &["public_key", "credential_id", "sign_count"])
            .await?;

        let Some(public_key) = public_key else {
            return Err(UserQueryError::NotFound);
        };

        let credential_id = credential_id.filter(|s| !s.is_empty());
        let sign_count_raw = sign_count_str.as_deref().unwrap_or("0");
        let sign_count = sign_count_raw.parse::<u32>().map_err(|_| {
            UserQueryError::DataCorruption(format!(
                "sign_count is not a valid u32: {sign_count_raw:?}"
            ))
        })?;

        Ok(Credential {
            fingerprint: fingerprint.to_owned(),
            public_key,
            credential_id,
            sign_count,
        })
    }

    pub async fn list_credentials(&mut self) -> Result<Vec<Credential>, UserQueryError> {
        let fingerprints: Vec<String> = self
            .user
            .client
            .conn
            .smembers(pubkey_key(&self.user.username))
            .await?;

        let mut credentials = Vec::with_capacity(fingerprints.len());
        for fp in &fingerprints {
            match self.get_credential(fp).await {
                Ok(cred) => credentials.push(cred),
                Err(UserQueryError::NotFound) => {
                    // Fingerprint exists in set but has no credential hash yet
                    // (legacy entry added via `add()` before credential support).
                    // Skip it in the credentials listing.
                }
                Err(e) => return Err(e),
            }
        }

        Ok(credentials)
    }

    pub async fn update_sign_count(
        &mut self,
        fingerprint: &str,
        sign_count: u32,
    ) -> Result<(), UserQueryError> {
        let cred_key = credential_key(&self.user.username, fingerprint);
        let _: () = self
            .user
            .client
            .conn
            .hset(&cred_key, "sign_count", sign_count.to_string())
            .await?;

        Ok(())
    }
}

/// Convert a COSE public key (from WebAuthn attestation) to an SSH-compatible
/// SHA-256 fingerprint. Constructs the SSH wire-format public key blob and hashes it.
///
/// Supports:
/// - COSE algorithm -7 (ES256 / ECDSA P-256): wire format is ecdsa-sha2-nistp256
/// - COSE algorithm -8 (EdDSA / Ed25519): wire format is ssh-ed25519
///
/// Returns the fingerprint string AND the OpenSSH-format public key string.
pub fn cose_key_to_ssh(cose_key_bytes: &[u8]) -> Result<(String, String), CoseKeyError> {
    use ciborium::Value;

    let value: Value =
        ciborium::from_reader(cose_key_bytes).map_err(|e| CoseKeyError::Cbor(e.to_string()))?;

    let map = match &value {
        Value::Map(m) => m,
        _ => return Err(CoseKeyError::Cbor("expected CBOR map".into())),
    };

    let get_int = |key: i64| -> Option<i64> {
        map.iter().find_map(|(k, v)| {
            let k_int = match k {
                Value::Integer(i) => i64::try_from(*i).ok()?,
                _ => return None,
            };
            if k_int != key {
                return None;
            }
            match v {
                Value::Integer(i) => i64::try_from(*i).ok(),
                _ => None,
            }
        })
    };

    let get_bytes = |key: i64| -> Option<&[u8]> {
        map.iter().find_map(|(k, v)| {
            let k_int = match k {
                Value::Integer(i) => i64::try_from(*i).ok()?,
                _ => return None,
            };
            if k_int != key {
                return None;
            }
            match v {
                Value::Bytes(b) => Some(b.as_slice()),
                _ => None,
            }
        })
    };

    let alg = get_int(3).ok_or(CoseKeyError::MissingParameter("alg (3)"))?;

    match alg {
        -7 => {
            // ES256 / ECDSA P-256
            let x = get_bytes(-2).ok_or(CoseKeyError::MissingParameter("x (-2)"))?;
            let y = get_bytes(-3).ok_or(CoseKeyError::MissingParameter("y (-3)"))?;

            if x.len() != 32 {
                return Err(CoseKeyError::InvalidKeyData(format!(
                    "EC P-256 x coordinate must be 32 bytes, got {}",
                    x.len()
                )));
            }
            if y.len() != 32 {
                return Err(CoseKeyError::InvalidKeyData(format!(
                    "EC P-256 y coordinate must be 32 bytes, got {}",
                    y.len()
                )));
            }

            // Build SEC1 uncompressed point: 0x04 || x || y
            let mut sec1_point = Vec::with_capacity(65);
            sec1_point.push(0x04);
            sec1_point.extend_from_slice(x);
            sec1_point.extend_from_slice(y);

            // Build SSH wire format for fingerprint computation
            let wire = build_ssh_wire_ecdsa_p256(&sec1_point);
            let fingerprint = compute_ssh_fingerprint(&wire);

            // Build OpenSSH format using ssh-key crate
            let ec_key = ssh_key::public::EcdsaPublicKey::from_sec1_bytes(&sec1_point)
                .map_err(|e| CoseKeyError::InvalidKeyData(e.to_string()))?;
            let pubkey = PublicKey::from(ssh_key::public::KeyData::Ecdsa(ec_key));
            let openssh = pubkey
                .to_openssh()
                .map_err(|e| CoseKeyError::InvalidKeyData(e.to_string()))?;

            Ok((fingerprint, openssh))
        }
        -8 => {
            // EdDSA / Ed25519
            let x = get_bytes(-2).ok_or(CoseKeyError::MissingParameter("x (-2)"))?;

            if x.len() != 32 {
                return Err(CoseKeyError::InvalidKeyData(format!(
                    "Ed25519 public key must be 32 bytes, got {}",
                    x.len()
                )));
            }

            // Build SSH wire format for fingerprint computation
            let wire = build_ssh_wire_ed25519(x);
            let fingerprint = compute_ssh_fingerprint(&wire);

            // Build OpenSSH format using ssh-key crate
            let ed_key = ssh_key::public::Ed25519PublicKey::try_from(x)
                .map_err(|e| CoseKeyError::InvalidKeyData(e.to_string()))?;
            let pubkey = PublicKey::from(ssh_key::public::KeyData::Ed25519(ed_key));
            let openssh = pubkey
                .to_openssh()
                .map_err(|e| CoseKeyError::InvalidKeyData(e.to_string()))?;

            Ok((fingerprint, openssh))
        }
        other => Err(CoseKeyError::UnsupportedAlgorithm(other)),
    }
}

fn ssh_wire_string(buf: &mut Vec<u8>, data: &[u8]) {
    buf.extend_from_slice(&(data.len() as u32).to_be_bytes());
    buf.extend_from_slice(data);
}

fn build_ssh_wire_ecdsa_p256(sec1_point: &[u8]) -> Vec<u8> {
    let mut wire = Vec::new();
    ssh_wire_string(&mut wire, b"ecdsa-sha2-nistp256");
    ssh_wire_string(&mut wire, b"nistp256");
    ssh_wire_string(&mut wire, sec1_point);
    wire
}

fn build_ssh_wire_ed25519(public_key: &[u8]) -> Vec<u8> {
    let mut wire = Vec::new();
    ssh_wire_string(&mut wire, b"ssh-ed25519");
    ssh_wire_string(&mut wire, public_key);
    wire
}

fn compute_ssh_fingerprint(wire_blob: &[u8]) -> String {
    let hash = sha2::Sha256::digest(wire_blob);
    let b64 = base64::engine::general_purpose::STANDARD_NO_PAD.encode(hash);
    format!("SHA256:{b64}")
}

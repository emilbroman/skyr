use base64::Engine;
use ed25519_dalek::SigningKey;
use rand::{RngCore, SeedableRng};
use redis::{AsyncCommands, Client as RedisClient};
use sha2::Digest;
use ssh_key::PublicKey;
use std::path::Path;
use std::time::{Duration, SystemTime};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConnectError {
    #[error("failed to create redis client: {0}")]
    RedisClient(#[from] redis::RedisError),

    #[error("failed to connect to redis server: {0}")]
    RedisConnection(#[source] redis::RedisError),
}

/// Per-region identity used to mint signed identity tokens.
///
/// The `region` is the issuer claim stamped onto every token this UDB
/// produces; `signing_key` is the corresponding Ed25519 private key. The
/// public counterpart is exposed by IAS via the `GetVerifyingKey` RPC,
/// where other regions' API edges fetch and cache it for token
/// verification.
#[derive(Clone)]
pub struct SigningIdentity {
    pub region: ids::RegionId,
    pub signing_key: SigningKey,
}

impl SigningIdentity {
    /// Load a signing identity from a 32-byte raw secret-key file.
    ///
    /// The file format is intentionally minimal: 32 bytes of Ed25519 secret
    /// scalar with no encoding wrapper. Operators generate one with e.g.
    /// `head -c 32 /dev/urandom > udb-signing.key`.
    pub fn load(
        region: ids::RegionId,
        path: impl AsRef<Path>,
    ) -> Result<Self, LoadSigningIdentityError> {
        let bytes = std::fs::read(path.as_ref()).map_err(LoadSigningIdentityError::Read)?;
        if bytes.len() != 32 {
            return Err(LoadSigningIdentityError::InvalidKeyLength(bytes.len()));
        }
        let key_bytes: [u8; 32] = bytes.try_into().expect("length checked above");
        Ok(Self {
            region,
            signing_key: SigningKey::from_bytes(&key_bytes),
        })
    }
}

#[derive(Error, Debug)]
pub enum LoadSigningIdentityError {
    #[error("failed to read signing key file: {0}")]
    Read(#[source] std::io::Error),

    #[error("signing key file must be exactly 32 bytes (got {0})")]
    InvalidKeyLength(usize),
}

#[derive(Error, Debug)]
pub enum IssueIdentityTokenError {
    #[error("UDB has no signing identity configured")]
    NoSigningIdentity,

    #[error("system clock error: {0}")]
    Clock(#[from] std::time::SystemTimeError),
}

#[derive(Default)]
pub struct ClientBuilder {
    known_nodes: Vec<String>,
    signing_identity: Option<SigningIdentity>,
}

impl ClientBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn known_node(mut self, hostname: impl AsRef<str>) -> Self {
        self.known_nodes.push(hostname.as_ref().to_owned());
        self
    }

    /// Attach a [`SigningIdentity`] so this UDB can issue identity tokens
    /// (see [`Client::issue_identity_token`]). Without one, identity-token
    /// issuance returns [`IssueIdentityTokenError::NoSigningIdentity`].
    pub fn signing_identity(mut self, identity: SigningIdentity) -> Self {
        self.signing_identity = Some(identity);
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
            signing_identity: self.signing_identity.clone(),
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

    #[error("invalid email: {0}")]
    InvalidEmail(String),
}

#[derive(Error, Debug)]
pub enum UserQueryError {
    #[error("failed to execute query: {0}")]
    Redis(#[from] redis::RedisError),

    #[error("user not found")]
    NotFound,

    #[error("invalid SSH public key: {0}")]
    InvalidPublicKey(#[from] ssh_key::Error),

    #[error("invalid username: {0}")]
    InvalidUsername(String),

    #[error("corrupted data: {0}")]
    DataCorruption(String),
}

#[derive(Error, Debug)]
pub enum CreateOrgError {
    #[error("failed to execute query: {0}")]
    Redis(#[from] redis::RedisError),

    #[error("name already taken")]
    NameTaken,

    #[error("invalid organization name: {0}")]
    InvalidName(String),

    #[error("creator user does not exist")]
    CreatorNotFound,
}

#[derive(Error, Debug)]
pub enum OrgQueryError {
    #[error("failed to execute query: {0}")]
    Redis(#[from] redis::RedisError),

    #[error("organization not found")]
    NotFound,

    #[error("user not found")]
    UserNotFound,

    #[error("user is already a member")]
    AlreadyMember,
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

#[derive(Debug, Clone)]
pub struct Org {
    pub name: String,
    pub creator: String,
}

#[derive(Clone)]
pub struct Client {
    conn: redis::aio::MultiplexedConnection,
    signing_identity: Option<SigningIdentity>,
}

fn user_key(username: &str) -> String {
    format!("u:{username}")
}

fn pubkey_key(username: &str) -> String {
    format!("p:{username}")
}

fn credential_key(username: &str, fingerprint: &str) -> String {
    format!("c:{username}:{fingerprint}")
}

fn org_key(orgname: &str) -> String {
    format!("o:{orgname}")
}

fn org_members_key(orgname: &str) -> String {
    format!("m:{orgname}")
}

fn user_orgs_key(username: &str) -> String {
    format!("om:{username}")
}

fn namespace_key(name: &str) -> String {
    format!("ns:{name}")
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

/// Validates that an organization name is safe for use in Redis key construction.
fn validate_orgname(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("organization name must not be empty".into());
    }
    if name.contains(':') {
        return Err("organization name must not contain ':'".into());
    }
    Ok(())
}

/// Validates that an email address has a basic valid format (non-empty local
/// part, `@`, and non-empty domain with at least one dot).
fn validate_email(email: &str) -> Result<(), String> {
    let Some((local, domain)) = email.split_once('@') else {
        return Err("email must contain '@'".into());
    };
    if local.is_empty() {
        return Err("email local part must not be empty".into());
    }
    if domain.is_empty() || !domain.contains('.') {
        return Err("email domain must contain at least one '.'".into());
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

    pub fn org(&self, name: impl Into<String>) -> OrgClient {
        OrgClient {
            client: self.clone(),
            name: name.into(),
        }
    }

    /// Public-key bytes of the configured signing identity, if any.
    ///
    /// Used by IAS to serve `GetVerifyingKey`, so other regions' API edges
    /// can verify tokens this UDB issues.
    pub fn signing_public_key(&self) -> Option<[u8; 32]> {
        self.signing_identity
            .as_ref()
            .map(|id| id.signing_key.verifying_key().to_bytes())
    }

    /// Mint a signed identity token for `username` valid for `ttl`.
    ///
    /// The issuer region is taken from the configured [`SigningIdentity`].
    /// The nonce is freshly drawn from the OS CSPRNG on every call.
    pub fn issue_identity_token(
        &self,
        username: &str,
        ttl: Duration,
    ) -> Result<String, IssueIdentityTokenError> {
        let identity = self
            .signing_identity
            .as_ref()
            .ok_or(IssueIdentityTokenError::NoSigningIdentity)?;

        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)?
            .as_secs() as i64;
        let mut nonce = [0u8; auth_token::NONCE_LEN];
        rand::rngs::StdRng::from_os_rng().fill_bytes(&mut nonce);

        let claims = auth_token::Claims {
            username: username.to_owned(),
            issuer_region: identity.region.clone(),
            issued_at: now,
            expires_at: now + ttl.as_secs() as i64,
            nonce,
        };
        Ok(auth_token::issue(&identity.signing_key, &claims))
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

    /// Mint a signed identity token for this user, valid for `ttl`.
    /// Forwards to [`Client::issue_identity_token`]; the surrounding
    /// `Client` must have a configured [`SigningIdentity`] whose region
    /// matches this user's home region.
    pub fn issue_identity_token(&self, ttl: Duration) -> Result<String, IssueIdentityTokenError> {
        self.client.issue_identity_token(&self.username, ttl)
    }

    pub async fn register(
        &self,
        email: impl Into<String>,
        fullname: Option<String>,
    ) -> Result<User, RegisterUserError> {
        validate_username(&self.username).map_err(RegisterUserError::InvalidUsername)?;
        let email = email.into();
        validate_email(&email).map_err(RegisterUserError::InvalidEmail)?;

        // Atomically check namespace reservation and create user.
        // Returns 0 on success, 1 if namespace is taken (org exists with that name),
        // 2 if username is already taken (user hash already exists).
        let script = redis::Script::new(
            r#"
            if redis.call('EXISTS', KEYS[1]) == 1 then
                return 1
            end
            local created = redis.call('HSETNX', KEYS[2], 'email', ARGV[1])
            if created == 0 then
                return 2
            end
            redis.call('SET', KEYS[1], 'user')
            return 0
            "#,
        );
        let mut conn = self.client.conn.clone();
        let result: i32 = script
            .key(namespace_key(&self.username))
            .key(user_key(&self.username))
            .arg(&email)
            .invoke_async(&mut conn)
            .await?;

        match result {
            1 | 2 => return Err(RegisterUserError::UsernameTaken),
            0 => {}
            _ => return Err(RegisterUserError::UsernameTaken),
        }

        if let Some(ref fullname) = fullname {
            let _: () = conn
                .hset(user_key(&self.username), "fullname", fullname)
                .await?;
        }

        Ok(User {
            username: self.username.clone(),
            email,
            fullname,
        })
    }

    pub async fn list_orgs(&self) -> Result<Vec<String>, UserQueryError> {
        let mut conn = self.client.conn.clone();
        let orgs: Vec<String> = conn.smembers(user_orgs_key(&self.username)).await?;
        Ok(orgs)
    }

    pub async fn set_fullname(&self, fullname: impl Into<String>) -> Result<(), UserQueryError> {
        let fullname = fullname.into();
        let key = user_key(&self.username);
        let mut conn = self.client.conn.clone();

        if fullname.is_empty() {
            let _: () = conn.hdel(&key, "fullname").await?;
        } else {
            let _: () = conn.hset(&key, "fullname", &fullname).await?;
        }

        Ok(())
    }

    pub async fn get(&self) -> Result<User, UserQueryError> {
        let mut conn = self.client.conn.clone();
        let (email, fullname): (Option<String>, Option<String>) = conn
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
pub struct PubkeysClient {
    user: UserClient,
}

impl PubkeysClient {
    pub async fn list(&self) -> Result<Vec<String>, UserQueryError> {
        let mut conn = self.user.client.conn.clone();
        let members: Vec<String> = conn.smembers(pubkey_key(&self.user.username)).await?;

        Ok(members)
    }

    pub async fn add(&self, pubkey: impl Into<String>) -> Result<(), UserQueryError> {
        let pubkey = pubkey.into();

        let mut conn = self.user.client.conn.clone();
        let _: () = conn.sadd(pubkey_key(&self.user.username), &pubkey).await?;

        Ok(())
    }

    pub async fn contains(&self, pubkey: impl Into<String>) -> Result<bool, UserQueryError> {
        let pubkey = pubkey.into();

        let mut conn = self.user.client.conn.clone();
        let contains: bool = conn
            .sismember(pubkey_key(&self.user.username), &pubkey)
            .await?;

        Ok(contains)
    }

    pub async fn remove(&self, pubkey: impl Into<String>) -> Result<(), UserQueryError> {
        let pubkey = pubkey.into();

        let mut conn = self.user.client.conn.clone();
        let _: () = conn.srem(pubkey_key(&self.user.username), &pubkey).await?;

        let _: () = conn
            .del(credential_key(&self.user.username, &pubkey))
            .await?;

        Ok(())
    }

    pub async fn add_credential(
        &self,
        public_key: &str,
        credential_id: Option<&str>,
        sign_count: u32,
    ) -> Result<Credential, UserQueryError> {
        let parsed = PublicKey::from_openssh(public_key)?;
        let fingerprint = parsed.fingerprint(ssh_key::HashAlg::Sha256).to_string();

        let mut conn = self.user.client.conn.clone();
        let _: () = conn
            .sadd(pubkey_key(&self.user.username), &fingerprint)
            .await?;

        let cred_key = credential_key(&self.user.username, &fingerprint);
        let cred_id_str = credential_id.unwrap_or("");

        redis::pipe()
            .hset(&cred_key, "public_key", public_key)
            .hset(&cred_key, "credential_id", cred_id_str)
            .hset(&cred_key, "sign_count", sign_count.to_string())
            .exec_async(&mut conn)
            .await?;

        Ok(Credential {
            fingerprint,
            public_key: public_key.to_owned(),
            credential_id: credential_id.map(|s| s.to_owned()),
            sign_count,
        })
    }

    pub async fn get_credential(&self, fingerprint: &str) -> Result<Credential, UserQueryError> {
        let cred_key = credential_key(&self.user.username, fingerprint);
        let mut conn = self.user.client.conn.clone();
        let (public_key, credential_id, sign_count_str): (
            Option<String>,
            Option<String>,
            Option<String>,
        ) = conn
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

    pub async fn list_credentials(&self) -> Result<Vec<Credential>, UserQueryError> {
        let mut conn = self.user.client.conn.clone();
        let fingerprints: Vec<String> = conn.smembers(pubkey_key(&self.user.username)).await?;

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
        &self,
        fingerprint: &str,
        sign_count: u32,
    ) -> Result<(), UserQueryError> {
        let cred_key = credential_key(&self.user.username, fingerprint);
        let mut conn = self.user.client.conn.clone();
        let _: () = conn
            .hset(&cred_key, "sign_count", sign_count.to_string())
            .await?;

        Ok(())
    }
}

#[derive(Clone)]
pub struct OrgClient {
    client: Client,
    name: String,
}

impl OrgClient {
    pub fn members(&self) -> OrgMembersClient {
        OrgMembersClient { org: self.clone() }
    }

    pub async fn create(&self, creator: &str) -> Result<Org, CreateOrgError> {
        validate_orgname(&self.name).map_err(CreateOrgError::InvalidName)?;

        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string();

        // Atomically: check namespace not taken, check creator exists, then create org.
        // Returns 0 on success, 1 if name taken, 2 if creator not found.
        let script = redis::Script::new(
            r#"
            if redis.call('EXISTS', KEYS[1]) == 1 then
                return 1
            end
            if redis.call('EXISTS', KEYS[2]) == 0 then
                return 2
            end
            redis.call('SET', KEYS[1], 'org')
            redis.call('HSET', KEYS[3], 'creator', ARGV[1], 'created_at', ARGV[2])
            redis.call('SADD', KEYS[4], ARGV[1])
            redis.call('SADD', KEYS[5], ARGV[3])
            return 0
            "#,
        );
        let mut conn = self.client.conn.clone();
        let result: i32 = script
            .key(namespace_key(&self.name))
            .key(user_key(creator))
            .key(org_key(&self.name))
            .key(org_members_key(&self.name))
            .key(user_orgs_key(creator))
            .arg(creator)
            .arg(&now)
            .arg(&self.name)
            .invoke_async(&mut conn)
            .await?;

        match result {
            0 => Ok(Org {
                name: self.name.clone(),
                creator: creator.to_owned(),
            }),
            1 => Err(CreateOrgError::NameTaken),
            2 => Err(CreateOrgError::CreatorNotFound),
            _ => Err(CreateOrgError::NameTaken),
        }
    }

    pub async fn get(&self) -> Result<Org, OrgQueryError> {
        let mut conn = self.client.conn.clone();
        let creator: Option<String> = conn.hget(org_key(&self.name), "creator").await?;

        let Some(creator) = creator else {
            return Err(OrgQueryError::NotFound);
        };

        Ok(Org {
            name: self.name.clone(),
            creator,
        })
    }
}

#[derive(Clone)]
pub struct OrgMembersClient {
    org: OrgClient,
}

impl OrgMembersClient {
    pub async fn list(&self) -> Result<Vec<String>, OrgQueryError> {
        let mut conn = self.org.client.conn.clone();
        let members: Vec<String> = conn.smembers(org_members_key(&self.org.name)).await?;
        Ok(members)
    }

    pub async fn add(&self, username: &str) -> Result<(), OrgQueryError> {
        let mut conn = self.org.client.conn.clone();

        // Verify the org exists
        let org_exists: bool = conn.exists(org_key(&self.org.name)).await?;
        if !org_exists {
            return Err(OrgQueryError::NotFound);
        }

        // Verify the user exists
        let user_exists: bool = conn.exists(user_key(username)).await?;
        if !user_exists {
            return Err(OrgQueryError::UserNotFound);
        }

        // Check if already a member
        let is_member: bool = conn
            .sismember(org_members_key(&self.org.name), username)
            .await?;
        if is_member {
            return Err(OrgQueryError::AlreadyMember);
        }

        // Add to both sets
        let _: () = conn.sadd(org_members_key(&self.org.name), username).await?;
        let _: () = conn.sadd(user_orgs_key(username), &self.org.name).await?;

        Ok(())
    }

    pub async fn contains(&self, username: &str) -> Result<bool, OrgQueryError> {
        let mut conn = self.org.client.conn.clone();
        let is_member: bool = conn
            .sismember(org_members_key(&self.org.name), username)
            .await?;
        Ok(is_member)
    }

    pub async fn remove(&self, username: &str) -> Result<(), OrgQueryError> {
        let mut conn = self.org.client.conn.clone();
        let _: () = conn.srem(org_members_key(&self.org.name), username).await?;
        let _: () = conn.srem(user_orgs_key(username), &self.org.name).await?;
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

    let kty = get_int(1).ok_or(CoseKeyError::MissingParameter("kty (1)"))?;
    let alg = get_int(3).ok_or(CoseKeyError::MissingParameter("alg (3)"))?;

    match alg {
        -7 => {
            // ES256 / ECDSA P-256 — kty must be 2 (EC2)
            if kty != 2 {
                return Err(CoseKeyError::InvalidKeyData(format!(
                    "ES256 key must have kty=2 (EC2), got {kty}"
                )));
            }
            // Validate curve parameter: crv must be 1 (P-256)
            let crv = get_int(-1).ok_or(CoseKeyError::MissingParameter("crv (-1)"))?;
            if crv != 1 {
                return Err(CoseKeyError::InvalidKeyData(format!(
                    "ES256 key must have crv=1 (P-256), got {crv}"
                )));
            }

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
            // EdDSA / Ed25519 — kty must be 1 (OKP)
            if kty != 1 {
                return Err(CoseKeyError::InvalidKeyData(format!(
                    "EdDSA key must have kty=1 (OKP), got {kty}"
                )));
            }
            // Validate curve parameter: crv must be 6 (Ed25519)
            let crv = get_int(-1).ok_or(CoseKeyError::MissingParameter("crv (-1)"))?;
            if crv != 6 {
                return Err(CoseKeyError::InvalidKeyData(format!(
                    "EdDSA key must have crv=6 (Ed25519), got {crv}"
                )));
            }

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

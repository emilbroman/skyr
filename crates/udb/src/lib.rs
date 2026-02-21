use base64::Engine;
use rand::{Rng, SeedableRng};
use redis::{AsyncCommands, Client as RedisClient};
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
}

#[derive(Error, Debug)]
pub enum UserQueryError {
    #[error("failed to execute query: {0}")]
    Redis(#[from] redis::RedisError),

    #[error("user not found")]
    NotFound,
}

#[derive(Error, Debug)]
pub enum LookupTokenError {
    #[error("failed to execute query: {0}")]
    Redis(#[from] redis::RedisError),

    #[error("invalid token")]
    InvalidToken,
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

const PREFIX_USER: &str = "u:";
const PREFIX_PUBKEY: &str = "p:";
const PREFIX_TOKEN: &str = "t:";

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
        let username: Option<String> = self.conn.get(format!("{PREFIX_TOKEN}{}", &token)).await?;

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

    pub async fn register(&mut self, email: impl Into<String>) -> Result<User, RegisterUserError> {
        let email = email.into();

        let result: i32 = self
            .client
            .conn
            .hset_nx(format!("{PREFIX_USER}{}", &self.username), "email", &email)
            .await?;

        if result == 0 {
            return Err(RegisterUserError::UsernameTaken);
        }

        Ok(User {
            username: self.username.clone(),
            email,
            fullname: None,
        })
    }

    pub async fn set_fullname(
        &mut self,
        fullname: impl Into<String>,
    ) -> Result<(), UserQueryError> {
        let fullname = fullname.into();

        let result: i32 = self
            .client
            .conn
            .hset(
                format!("{PREFIX_USER}{}", &self.username),
                "fullname",
                &fullname,
            )
            .await?;

        if result == 0 {
            Err(UserQueryError::NotFound)
        } else {
            Ok(())
        }
    }

    pub async fn get(&mut self) -> Result<User, UserQueryError> {
        let (email, fullname) = self
            .client
            .conn
            .hmget(
                format!("{PREFIX_USER}{}", &self.username),
                &["email", "fullname"],
            )
            .await?;

        let Some(email) = email else {
            return Err(UserQueryError::NotFound);
        };

        Ok(User {
            username: self.username.clone(),
            email,
            fullname,
        })
    }
}

#[derive(Clone)]
pub struct TokensClient {
    user: UserClient,
}

impl TokensClient {
    pub async fn issue(&mut self) -> Result<String, UserQueryError> {
        let mut token = String::new();

        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode_string(self.user.client.rng.random::<[u8; 32]>(), &mut token);

        let _: () = self
            .user
            .client
            .conn
            .set_ex(
                format!("{PREFIX_TOKEN}{}", &token),
                &self.user.username,
                900,
            )
            .await?;

        Ok(token)
    }

    pub async fn revoke(&mut self, token: impl Into<String>) -> Result<(), LookupTokenError> {
        let token = token.into();

        let deleted: bool = redis::cmd("DELEX")
            .arg(format!("{PREFIX_TOKEN}{}", &token))
            .arg("IFEQ")
            .arg(&self.user.username)
            .query_async(&mut self.user.client.conn)
            .await?;

        if !deleted {
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
    pub async fn add(&mut self, pubkey: impl Into<String>) -> Result<(), UserQueryError> {
        let pubkey = pubkey.into();

        let _: () = self
            .user
            .client
            .conn
            .sadd(format!("{PREFIX_PUBKEY}{}", &self.user.username), &pubkey)
            .await?;

        Ok(())
    }

    pub async fn contains(&mut self, pubkey: impl Into<String>) -> Result<bool, UserQueryError> {
        let pubkey = pubkey.into();

        let contains: bool = self
            .user
            .client
            .conn
            .sismember(format!("{PREFIX_PUBKEY}{}", &self.user.username), &pubkey)
            .await?;

        Ok(contains)
    }

    pub async fn remove(&mut self, pubkey: impl Into<String>) -> Result<(), UserQueryError> {
        let pubkey = pubkey.into();

        let _: () = self
            .user
            .client
            .conn
            .srem(format!("{PREFIX_PUBKEY}{}", &self.user.username), &pubkey)
            .await?;

        Ok(())
    }
}

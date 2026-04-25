//! Redis-backed dedup store for notification idempotency.
//!
//! The Notification Queue (NQ) is at-least-once. To avoid sending the same user
//! a duplicate email when a delivery is redelivered (broker restart, NE crash
//! mid-handle, etc.), every send is gated by a Redis key derived from the
//! request's stable idempotency key — the pair `(incident_id, event_type)`
//! exposed by [`nq::NotificationRequest::idempotency_key`].
//!
//! The store provides three operations:
//!
//! - [`DedupStore::try_claim`] atomically inserts the key with a TTL **only if**
//!   it does not already exist. Returns `Claimed` on first attempt and
//!   `AlreadyClaimed` on a duplicate. The first NE that claims the key proceeds
//!   to send the email; everyone else acks the redelivery and stops.
//! - [`DedupStore::release`] deletes the key, used on transient SMTP failures
//!   so a subsequent redelivery can re-attempt the send. Without this, a
//!   transient SMTP failure followed by a successful redelivery would be
//!   silently dropped because the key was claimed before the failed send.
//! - [`DedupStore::confirm`] is currently a no-op — Redis TTL on the claim is
//!   sufficient — but is exposed as an explicit step in case the dedup
//!   semantics ever evolve to a two-phase commit.
//!
//! # Why Redis (and not Scylla)
//!
//! Redis is already a load-bearing dependency in this codebase (`udb`, `scs`,
//! `scoc`, several plugins), making it the obvious fit. The pattern needed here
//! is a single key with a TTL and `SET NX`; spinning up a Scylla keyspace would
//! be heavy for a self-cleaning ledger. The brief explicitly preferred Redis
//! when one is already in use.

use redis::{AsyncCommands, Client as RedisClient};
use thiserror::Error;

const KEY_PREFIX: &str = "ne:dedup:";

/// Connection settings for the dedup store.
#[derive(Debug, Clone)]
pub struct DedupConfig {
    /// Hostname of the Redis server. The full URL is constructed as
    /// `redis://{hostname}/`.
    pub hostname: String,
    /// Time-to-live applied to every claim key. Once it expires, a fresh
    /// redelivery would be re-processed; choose a value comfortably greater
    /// than the longest plausible queue dwell time. Defaults to 7 days.
    pub ttl_seconds: u64,
}

impl Default for DedupConfig {
    fn default() -> Self {
        Self {
            hostname: "127.0.0.1".to_string(),
            ttl_seconds: 7 * 24 * 60 * 60,
        }
    }
}

/// Outcome of a [`DedupStore::try_claim`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimOutcome {
    /// The key was set; this caller is responsible for performing the send.
    Claimed,
    /// The key was already set by an earlier caller; the current delivery is a
    /// duplicate and must not be re-sent.
    AlreadyClaimed,
}

#[derive(Debug, Error)]
pub enum DedupError {
    #[error("failed to create redis client: {0}")]
    RedisClient(#[source] redis::RedisError),

    #[error("failed to connect to redis server: {0}")]
    RedisConnection(#[source] redis::RedisError),

    #[error("failed to execute redis command: {0}")]
    RedisCommand(#[source] redis::RedisError),

    #[error("dedup ttl_seconds must fit in u32, got {0}")]
    TtlOutOfRange(u64),
}

/// Redis-backed dedup store. Cheap to clone; sharing one instance across worker
/// tasks is fine because the underlying connection is multiplexed.
#[derive(Clone)]
pub struct DedupStore {
    conn: redis::aio::MultiplexedConnection,
    ttl_seconds: u64,
}

impl DedupStore {
    /// Connects to Redis and returns a ready-to-use store.
    pub async fn connect(config: &DedupConfig) -> Result<Self, DedupError> {
        let url = format!("redis://{}/", config.hostname);
        let client = RedisClient::open(url).map_err(DedupError::RedisClient)?;
        let conn = client
            .get_multiplexed_async_connection()
            .await
            .map_err(DedupError::RedisConnection)?;

        Ok(Self {
            conn,
            ttl_seconds: config.ttl_seconds,
        })
    }

    /// Attempts to claim the given idempotency key.
    ///
    /// Implementation: `SET key 1 EX ttl NX`. The Redis command returns the
    /// string "OK" when the key was newly set, and the nil reply otherwise.
    pub async fn try_claim(&self, idempotency_key: &str) -> Result<ClaimOutcome, DedupError> {
        let ttl: u64 = self.ttl_seconds;
        let key = make_key(idempotency_key);

        let mut conn = self.conn.clone();
        // redis::AsyncCommands does not currently expose `SET ... NX EX`
        // directly; build the command via the lower-level `cmd()` API.
        let result: Option<String> = redis::cmd("SET")
            .arg(&key)
            .arg("1")
            .arg("EX")
            .arg(ttl)
            .arg("NX")
            .query_async(&mut conn)
            .await
            .map_err(DedupError::RedisCommand)?;

        Ok(match result {
            Some(_) => ClaimOutcome::Claimed,
            None => ClaimOutcome::AlreadyClaimed,
        })
    }

    /// Releases a previously claimed key, allowing a future redelivery to
    /// retry. Call this only after a transient send failure where it is
    /// strictly preferable to retry than to silently drop the notification.
    pub async fn release(&self, idempotency_key: &str) -> Result<(), DedupError> {
        let key = make_key(idempotency_key);
        let mut conn = self.conn.clone();
        let _: i64 = conn.del(&key).await.map_err(DedupError::RedisCommand)?;
        Ok(())
    }

    /// Marks a claim as definitively consumed. Currently a no-op (the TTL on
    /// the original `SET NX` is sufficient), but kept as an explicit hook so
    /// callers can express the lifecycle even if the implementation later
    /// changes.
    pub async fn confirm(&self, _idempotency_key: &str) -> Result<(), DedupError> {
        Ok(())
    }
}

fn make_key(idempotency_key: &str) -> String {
    format!("{KEY_PREFIX}{idempotency_key}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_key_prefixes_with_namespace() {
        assert_eq!(
            make_key("01HZX9P5K2JN7YQVJ3Q6T4ZB8N:opened"),
            "ne:dedup:01HZX9P5K2JN7YQVJ3Q6T4ZB8N:opened"
        );
    }

    #[test]
    fn default_ttl_is_seven_days() {
        let cfg = DedupConfig::default();
        assert_eq!(cfg.ttl_seconds, 7 * 24 * 60 * 60);
    }
}

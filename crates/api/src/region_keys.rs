//! In-process cache of regions' identity-token signing public keys.
//!
//! The keys themselves live in GDDB's `region_keys` table (see the
//! `gddb` crate). Every authenticated request needs at least one key
//! lookup, so we cache them here with a short TTL. Rotations propagate
//! by TTL expiry; a verification failure also invalidates the cached
//! entry so the next request refetches eagerly (defense-in-depth for
//! the moment between rotation and TTL expiry).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ed25519_dalek::VerifyingKey;
use ids::RegionId;
use thiserror::Error;
use tokio::sync::Mutex;

/// How long a cached region key is trusted before re-fetching from GDDB.
/// Matches the architecture doc's "rotations propagate by TTL" stance.
const DEFAULT_TTL: Duration = Duration::from_secs(300);

#[derive(Clone)]
pub(crate) struct RegionKeyCache {
    inner: Arc<Mutex<HashMap<RegionId, CachedKey>>>,
    gddb: gddb::Client,
    ttl: Duration,
}

struct CachedKey {
    key: VerifyingKey,
    fetched_at: Instant,
}

impl RegionKeyCache {
    pub(crate) fn new(gddb: gddb::Client) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            gddb,
            ttl: DEFAULT_TTL,
        }
    }

    /// Look up `region`'s identity-token signing public key. Returns the
    /// cached value if it is still fresh, otherwise fetches from GDDB and
    /// caches the result.
    pub(crate) async fn get(&self, region: &RegionId) -> Result<VerifyingKey, FetchError> {
        {
            let entries = self.inner.lock().await;
            if let Some(entry) = entries.get(region)
                && entry.fetched_at.elapsed() < self.ttl
            {
                return Ok(entry.key);
            }
        }

        let row = self
            .gddb
            .lookup_region_key(region)
            .await?
            .ok_or_else(|| FetchError::Unknown(region.clone()))?;

        if row.public_key.len() != 32 {
            return Err(FetchError::InvalidKeyLength(row.public_key.len()));
        }
        let key_bytes: [u8; 32] = row.public_key.try_into().expect("length checked above");
        let key =
            VerifyingKey::from_bytes(&key_bytes).map_err(|_| FetchError::InvalidEd25519Key)?;

        self.inner.lock().await.insert(
            region.clone(),
            CachedKey {
                key,
                fetched_at: Instant::now(),
            },
        );

        Ok(key)
    }

    /// Drop the cached entry for `region`. Called after a verification
    /// failure so the next attempt re-reads from GDDB; legitimate rotations
    /// then take effect immediately for that region.
    pub(crate) async fn invalidate(&self, region: &RegionId) {
        self.inner.lock().await.remove(region);
    }
}

#[derive(Error, Debug)]
pub(crate) enum FetchError {
    #[error("failed to look up region key in GDDB: {0}")]
    Lookup(#[from] gddb::LookupError),

    #[error("region {0} has no published signing key")]
    Unknown(RegionId),

    #[error("region key in GDDB has invalid length: {0}")]
    InvalidKeyLength(usize),

    #[error("region key in GDDB is not a valid Ed25519 public key")]
    InvalidEd25519Key,
}

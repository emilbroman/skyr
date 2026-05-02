//! In-process cache of regions' identity-token signing public keys.
//!
//! Keys are fetched on demand from the issuing region's IAS (via gRPC
//! `GetVerifyingKey`). Every authenticated request needs at least one
//! key lookup, so we cache them here with a short TTL. Rotations
//! propagate by TTL expiry; a verification failure also invalidates the
//! cached entry so the next request refetches eagerly (defense-in-depth
//! for the moment between rotation and TTL expiry).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ed25519_dalek::VerifyingKey;
use ids::RegionId;
use thiserror::Error;
use tokio::sync::Mutex;

use crate::pools::{IasConnectError, IasPool};

/// How long a cached region key is trusted before re-fetching from IAS.
/// Matches the architecture doc's "rotations propagate by TTL" stance.
const DEFAULT_TTL: Duration = Duration::from_secs(300);

#[derive(Clone)]
pub(crate) struct RegionKeyCache {
    inner: Arc<Mutex<HashMap<RegionId, CachedKey>>>,
    ias_pool: IasPool,
    ttl: Duration,
}

struct CachedKey {
    key: VerifyingKey,
    fetched_at: Instant,
}

impl RegionKeyCache {
    pub(crate) fn new(ias_pool: IasPool) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            ias_pool,
            ttl: DEFAULT_TTL,
        }
    }

    /// Look up `region`'s identity-token signing public key. Returns the
    /// cached value if it is still fresh, otherwise fetches from the
    /// region's IAS and caches the result.
    pub(crate) async fn get(&self, region: &RegionId) -> Result<VerifyingKey, FetchError> {
        {
            let entries = self.inner.lock().await;
            if let Some(entry) = entries.get(region)
                && entry.fetched_at.elapsed() < self.ttl
            {
                return Ok(entry.key);
            }
        }

        let mut client = self.ias_pool.for_region(region).await?;
        let response = match client.get_verifying_key(()).await {
            Ok(resp) => resp.into_inner(),
            Err(status) if status.code() == tonic::Code::NotFound => {
                return Err(FetchError::Unknown(region.clone()));
            }
            Err(status) => return Err(FetchError::Rpc(status)),
        };

        if response.public_key.len() != 32 {
            return Err(FetchError::InvalidKeyLength(response.public_key.len()));
        }
        let key_bytes: [u8; 32] = response
            .public_key
            .try_into()
            .expect("length checked above");
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
    /// failure so the next attempt re-reads from IAS; legitimate rotations
    /// then take effect immediately for that region.
    pub(crate) async fn invalidate(&self, region: &RegionId) {
        self.inner.lock().await.remove(region);
    }
}

#[derive(Error, Debug)]
pub(crate) enum FetchError {
    #[error("failed to connect to IAS: {0}")]
    Connect(#[from] IasConnectError),

    #[error("IAS RPC failed: {0}")]
    Rpc(#[source] tonic::Status),

    #[error("region {0} has no published signing key")]
    Unknown(RegionId),

    #[error("region key from IAS has invalid length: {0}")]
    InvalidKeyLength(usize),

    #[error("region key from IAS is not a valid Ed25519 public key")]
    InvalidEd25519Key,
}

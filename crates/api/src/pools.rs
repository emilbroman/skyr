//! Per-region connection pools for UDB / CDB / SDB.
//!
//! Each pool holds a `HashMap<RegionId, Client>` populated lazily on first
//! use of a region. Construction is parameterized by the regional service
//! address scheme (`<service>.<region>.int.<domain>`), so adding a region
//! is operator data — no Skyr-binary change.
//!
//! In stage 4 of the L7-routing rollout the API still has `--region` and
//! still rejects requests that target a different home region, so each
//! pool effectively only ever holds the local region's client. The pools
//! land here so stage 5 (when the rejection drops) is a one-line change
//! per resolver rather than a fresh refactor.

use std::collections::HashMap;
use std::sync::Arc;

use ids::{Domain, RegionId, service_address};
use tokio::sync::Mutex;

#[derive(Clone)]
pub(crate) struct UdbPool {
    inner: Arc<Mutex<HashMap<RegionId, udb::Client>>>,
    domain: Domain,
    /// The local region's signing identity, attached only when this pool
    /// hands out the local UDB client. Other regions sign with their own
    /// keys via their own API edges; we only verify their tokens here.
    local_signing_identity: Option<udb::SigningIdentity>,
}

impl UdbPool {
    pub(crate) fn new(
        domain: Domain,
        local_signing_identity: Option<udb::SigningIdentity>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            domain,
            local_signing_identity,
        }
    }

    pub(crate) async fn for_region(
        &self,
        region: &RegionId,
    ) -> Result<udb::Client, udb::ConnectError> {
        {
            let entries = self.inner.lock().await;
            if let Some(client) = entries.get(region) {
                return Ok(client.clone());
            }
        }

        let mut builder =
            udb::ClientBuilder::new().known_node(service_address("udb", region, &self.domain));
        if let Some(identity) = &self.local_signing_identity
            && &identity.region == region
        {
            builder = builder.signing_identity(identity.clone());
        }
        let client = builder.build().await?;

        self.inner
            .lock()
            .await
            .insert(region.clone(), client.clone());
        Ok(client)
    }
}

#[derive(Clone)]
pub(crate) struct CdbPool {
    inner: Arc<Mutex<HashMap<RegionId, cdb::Client>>>,
    domain: Domain,
}

impl CdbPool {
    pub(crate) fn new(domain: Domain) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            domain,
        }
    }

    pub(crate) async fn for_region(
        &self,
        region: &RegionId,
    ) -> Result<cdb::Client, cdb::ConnectError> {
        {
            let entries = self.inner.lock().await;
            if let Some(client) = entries.get(region) {
                return Ok(client.clone());
            }
        }

        let client = cdb::ClientBuilder::new()
            .known_node(service_address("cdb", region, &self.domain))
            .build()
            .await?;

        self.inner
            .lock()
            .await
            .insert(region.clone(), client.clone());
        Ok(client)
    }
}

#[derive(Clone)]
pub(crate) struct SdbPool {
    inner: Arc<Mutex<HashMap<RegionId, sdb::Client>>>,
    domain: Domain,
}

impl SdbPool {
    pub(crate) fn new(domain: Domain) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            domain,
        }
    }

    pub(crate) async fn for_region(
        &self,
        region: &RegionId,
    ) -> Result<sdb::Client, sdb::ConnectError> {
        {
            let entries = self.inner.lock().await;
            if let Some(client) = entries.get(region) {
                return Ok(client.clone());
            }
        }

        let client = sdb::ClientBuilder::new()
            .known_node(service_address("sdb", region, &self.domain))
            .build()
            .await?;

        self.inner
            .lock()
            .await
            .insert(region.clone(), client.clone());
        Ok(client)
    }
}

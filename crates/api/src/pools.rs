//! Per-region connection pools for IAS / CDB / SDB.
//!
//! Each pool holds a `HashMap<RegionId, Client>` populated lazily on first
//! use of a region. Construction is parameterized by the regional service
//! address scheme (`<service>.<region>.int.<domain>`), so adding a region
//! is operator data — no Skyr-binary change.

use std::collections::HashMap;
use std::sync::Arc;

use ids::{Domain, RegionId, service_address};
use thiserror::Error;
use tokio::sync::Mutex;
use tonic::transport::Endpoint;

/// Default IAS gRPC port. Mirrored on the IAS binary's `--port` default.
const IAS_PORT: u16 = 50100;

#[derive(Error, Debug)]
pub(crate) enum IasConnectError {
    #[error("invalid IAS endpoint: {0}")]
    InvalidEndpoint(#[from] tonic::transport::Error),
}

#[derive(Clone)]
pub(crate) struct IasPool {
    inner: Arc<Mutex<HashMap<RegionId, ias::IdentityAndAccessClient>>>,
    domain: Domain,
}

impl IasPool {
    pub(crate) fn new(domain: Domain) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            domain,
        }
    }

    pub(crate) async fn for_region(
        &self,
        region: &RegionId,
    ) -> Result<ias::IdentityAndAccessClient, IasConnectError> {
        {
            let entries = self.inner.lock().await;
            if let Some(client) = entries.get(region) {
                return Ok(client.clone());
            }
        }

        let host = service_address("ias", region, &self.domain);
        let endpoint = Endpoint::from_shared(format!("http://{host}:{IAS_PORT}"))?;
        // Connect lazily so a transient unavailability of one region's IAS
        // doesn't fail unrelated requests on this edge.
        let channel = endpoint.connect_lazy();
        let client = ias::IdentityAndAccessClient::new(channel);

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

//! Per-region service connection pools.
//!
//! Mirrors `api/src/pools.rs`: SCS is a region-agnostic edge that routes
//! per-channel to whichever region owns the data — IAS at the user's
//! home for SSH pubkey checks, CDB at the repo's home for git
//! receive-pack/upload-pack, RDB at the resource's region for
//! port-forward, and the per-region SCOC node registry (a Redis instance)
//! at that same region. Connections open lazily on first use of a region
//! and are cached for the process lifetime.

use std::collections::HashMap;
use std::sync::Arc;

use ids::{RegionId, ServiceAddressTemplate};
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
    template: ServiceAddressTemplate,
}

impl IasPool {
    pub(crate) fn new(template: ServiceAddressTemplate) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            template,
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

        let host = self.template.format("ias", region);
        let endpoint = Endpoint::from_shared(format!("http://{host}:{IAS_PORT}"))?;
        // Lazy connect: a transient unavailability of one region's IAS
        // shouldn't fail unrelated SSH sessions on this edge.
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
    template: ServiceAddressTemplate,
}

impl CdbPool {
    pub(crate) fn new(template: ServiceAddressTemplate) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            template,
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
            .known_node(self.template.format("cdb", region))
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
pub(crate) struct RdbPool {
    inner: Arc<Mutex<HashMap<RegionId, rdb::Client>>>,
    template: ServiceAddressTemplate,
}

impl RdbPool {
    pub(crate) fn new(template: ServiceAddressTemplate) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            template,
        }
    }

    pub(crate) async fn for_region(
        &self,
        region: &RegionId,
    ) -> Result<rdb::Client, rdb::ConnectError> {
        {
            let entries = self.inner.lock().await;
            if let Some(client) = entries.get(region) {
                return Ok(client.clone());
            }
        }

        let client = rdb::ClientBuilder::new()
            .known_node(self.template.format("rdb", region))
            .region(region.clone())
            .build()
            .await?;

        self.inner
            .lock()
            .await
            .insert(region.clone(), client.clone());
        Ok(client)
    }
}

/// Per-region pool of connections to the SCOC node-registry Redis.
///
/// Each region's user-workload fleet has its own SCOC instance and its
/// own node-registry Redis; an SCS edge servicing a port-forward needs
/// to consult the registry in the resource's region (encoded in the
/// resource's QID).
#[derive(Clone)]
pub(crate) struct NodeRegistryPool {
    inner: Arc<Mutex<HashMap<RegionId, redis::aio::MultiplexedConnection>>>,
    template: ServiceAddressTemplate,
}

impl NodeRegistryPool {
    pub(crate) fn new(template: ServiceAddressTemplate) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            template,
        }
    }

    pub(crate) async fn for_region(
        &self,
        region: &RegionId,
    ) -> Result<redis::aio::MultiplexedConnection, redis::RedisError> {
        {
            let entries = self.inner.lock().await;
            if let Some(conn) = entries.get(region) {
                return Ok(conn.clone());
            }
        }

        let host = self.template.format("node-registry", region);
        let url = format!("redis://{host}/");
        let conn = redis::Client::open(url)?
            .get_multiplexed_async_connection()
            .await?;

        self.inner.lock().await.insert(region.clone(), conn.clone());
        Ok(conn)
    }
}

//! Per-region connection pools for IAS / CDB / SDB / RDB / LDB.
//!
//! Each pool holds a `HashMap<RegionId, Client>` populated lazily on first
//! use of a region. Construction is parameterized by the regional service
//! address template (e.g. `{service}.{region}.int.skyr.cloud`), so adding a
//! region is operator data — no Skyr-binary change.

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
pub(crate) struct SdbPool {
    inner: Arc<Mutex<HashMap<RegionId, sdb::Client>>>,
    template: ServiceAddressTemplate,
}

impl SdbPool {
    pub(crate) fn new(template: ServiceAddressTemplate) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            template,
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
            .known_node(self.template.format("sdb", region))
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

/// Default LDB Kafka broker port. Mirrors the LDB compose/k8s configuration.
const LDB_PORT: u16 = 9092;

#[derive(Clone)]
pub(crate) struct LdbConsumerPool {
    inner: Arc<Mutex<HashMap<RegionId, ldb::Consumer>>>,
    template: ServiceAddressTemplate,
}

impl LdbConsumerPool {
    pub(crate) fn new(template: ServiceAddressTemplate) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            template,
        }
    }

    pub(crate) async fn for_region(
        &self,
        region: &RegionId,
    ) -> Result<ldb::Consumer, ldb::ConnectError> {
        {
            let entries = self.inner.lock().await;
            if let Some(client) = entries.get(region) {
                return Ok(client.clone());
            }
        }

        let brokers = format!("{}:{LDB_PORT}", self.template.format("ldb", region));
        let client = ldb::ClientBuilder::new()
            .brokers(brokers)
            .build_consumer()
            .await?;

        self.inner
            .lock()
            .await
            .insert(region.clone(), client.clone());
        Ok(client)
    }
}

#[derive(Clone)]
pub(crate) struct LdbPublisherPool {
    inner: Arc<Mutex<HashMap<RegionId, ldb::Publisher>>>,
    template: ServiceAddressTemplate,
}

impl LdbPublisherPool {
    pub(crate) fn new(template: ServiceAddressTemplate) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            template,
        }
    }

    pub(crate) async fn for_region(
        &self,
        region: &RegionId,
    ) -> Result<ldb::Publisher, ldb::ConnectError> {
        {
            let entries = self.inner.lock().await;
            if let Some(client) = entries.get(region) {
                return Ok(client.clone());
            }
        }

        let brokers = format!("{}:{LDB_PORT}", self.template.format("ldb", region));
        let client = ldb::ClientBuilder::new()
            .brokers(brokers)
            .build_publisher()
            .await?;

        self.inner
            .lock()
            .await
            .insert(region.clone(), client.clone());
        Ok(client)
    }
}

//! Per-region RDB connection pool used by the DE to read cross-region
//! dependency state during evaluation.
//!
//! The DE in a repo's home region is the only orchestrator that ever reads
//! resources from another region's RDB. Per the architecture spec, those
//! reads are batched one query per remote region per evaluation iteration.
//! This pool holds a `HashMap<RegionId, rdb::Client>` populated lazily on
//! first use of a region; the local region's client is pre-seeded so the
//! single-region case never opens a second connection.

use std::collections::HashMap;
use std::sync::Arc;

use ids::{RegionId, ServiceAddressTemplate};
use tokio::sync::Mutex;

#[derive(Clone)]
pub(crate) struct RdbPool {
    inner: Arc<Mutex<HashMap<RegionId, rdb::Client>>>,
    template: ServiceAddressTemplate,
}

impl RdbPool {
    /// Builds a pool pre-seeded with the local region's client. `local_region`
    /// must match the region the client was built for; the client's
    /// `region()` accessor is the source of truth.
    pub(crate) fn new(template: ServiceAddressTemplate, local: rdb::Client) -> Self {
        let mut entries = HashMap::new();
        entries.insert(local.region().clone(), local);
        Self {
            inner: Arc::new(Mutex::new(entries)),
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

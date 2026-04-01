//! BB3-specific internet address allocation.
//!
//! This module talks to the BB3 homelab address service to allocate and release
//! floating public IPs paired with LAN VIPs. The implementation is intentionally
//! isolated here so it can be replaced with a generic provider later.

use anyhow::{Context, bail};
use serde::Deserialize;
use tracing::{info, warn};

/// Base URL of the BB3 address allocation service.
const BB3_ADDR_SERVICE_URL: &str = "http://192.168.2.4:8080";

/// An allocated internet address pair.
pub struct AddressAllocation {
    /// The public floating IP (visible to the outside world).
    pub floating_ip: String,
    /// The LAN virtual IP (used internally for NAT on the cluster node).
    pub lan_ip: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AllocateResponse {
    floating_ip: String,
    lan_ip: String,
}

/// Allocate a new internet address (floating IP + LAN VIP) from the BB3 service.
pub async fn allocate_address() -> anyhow::Result<AddressAllocation> {
    let url = format!("{BB3_ADDR_SERVICE_URL}/addr");
    info!(url = %url, "allocating internet address from BB3 service");

    let response = reqwest::Client::new()
        .post(&url)
        .send()
        .await
        .context("POST to BB3 address service failed")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        bail!("BB3 address allocation failed (HTTP {status}): {body}");
    }

    let alloc: AllocateResponse = response
        .json()
        .await
        .context("failed to parse BB3 address allocation response")?;

    info!(
        floating_ip = %alloc.floating_ip,
        lan_ip = %alloc.lan_ip,
        "allocated internet address"
    );

    Ok(AddressAllocation {
        floating_ip: alloc.floating_ip,
        lan_ip: alloc.lan_ip,
    })
}

/// Release a previously allocated internet address back to the BB3 service.
pub async fn release_address(lan_ip: &str) -> anyhow::Result<()> {
    let url = format!("{BB3_ADDR_SERVICE_URL}/addr/{lan_ip}");
    info!(url = %url, lan_ip = %lan_ip, "releasing internet address to BB3 service");

    let response = reqwest::Client::new()
        .delete(&url)
        .send()
        .await
        .context("DELETE to BB3 address service failed")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        warn!(
            lan_ip = %lan_ip,
            status = %status,
            body = %body,
            "BB3 address release returned non-success status"
        );
    }

    Ok(())
}

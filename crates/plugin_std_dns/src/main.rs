use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use tracing::{debug, error};

mod dns_server;
mod dns_store;

const A_RECORD_RESOURCE_TYPE: &str = "Std/DNS.ARecord";

/// Normalize a string for use as a DNS label: lowercase and replace non-alphanumeric
/// characters with hyphens.
fn normalize_dns_label(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_lowercase()
}

#[derive(Parser)]
struct Args {
    #[arg(long)]
    bind: String,

    #[arg(long, default_value = "53")]
    dns_port: u16,

    #[arg(long)]
    redis_hostname: String,

    #[arg(long)]
    zone: String,
}

struct DnsPluginInner {
    store: dns_store::DnsStore,
    zone: String,
}

#[derive(Clone)]
struct DnsPlugin {
    inner: Arc<DnsPluginInner>,
}

impl DnsPlugin {
    fn new(store: dns_store::DnsStore, zone: String) -> Self {
        Self {
            inner: Arc::new(DnsPluginInner { store, zone }),
        }
    }

    fn fqdn(&self, name: &str, environment_qid: &ids::EnvironmentQid) -> String {
        let org = normalize_dns_label(environment_qid.repo.org.as_str());
        let repo = normalize_dns_label(environment_qid.repo.repo.as_str());
        let env = normalize_dns_label(environment_qid.environment.as_str());
        format!("{name}.{env}.{repo}.{org}.{}", self.inner.zone).to_lowercase()
    }

    fn extract_addresses(inputs: &sclc::Record) -> Vec<String> {
        match inputs.get("addresses") {
            sclc::Value::List(list) => list
                .iter()
                .filter_map(|v| match v {
                    sclc::Value::Str(s) => Some(s.clone()),
                    _ => None,
                })
                .collect(),
            _ => vec![],
        }
    }

    fn extract_ttl_seconds(inputs: &sclc::Record) -> u32 {
        let ttl = match inputs.get("ttl") {
            sclc::Value::Record(r) => r,
            _ => return 1,
        };
        let millis = match ttl.get("milliseconds") {
            sclc::Value::Int(ms) => *ms,
            _ => return 1,
        };
        (millis / 1000).clamp(1, u32::MAX as i64) as u32
    }

    async fn dispatch(
        &self,
        environment_qid: &ids::EnvironmentQid,
        id: &ids::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        match id.typ.as_str() {
            A_RECORD_RESOURCE_TYPE => {
                let name = match inputs.get("name") {
                    sclc::Value::Str(s) => s.clone(),
                    _ => anyhow::bail!("missing or invalid 'name' input"),
                };
                let addresses = Self::extract_addresses(&inputs);
                let ttl_seconds = Self::extract_ttl_seconds(&inputs);
                let fqdn = self.fqdn(&name, environment_qid);

                self.inner
                    .store
                    .set_a_record(&fqdn, &addresses, ttl_seconds)
                    .await?;

                let mut outputs = sclc::Record::default();
                outputs.insert(String::from("fqdn"), sclc::Value::Str(fqdn));
                outputs.insert(String::from("ttl"), inputs.get("ttl").clone());
                outputs.insert(String::from("addresses"), inputs.get("addresses").clone());

                Ok(sclc::Resource {
                    inputs,
                    outputs,
                    dependencies: vec![],
                    markers: Default::default(),
                })
            }
            _ => anyhow::bail!("unsupported resource type: {}", id.typ),
        }
    }
}

#[async_trait::async_trait]
impl rtp::Plugin for DnsPlugin {
    async fn create_resource(
        &mut self,
        deployment_qid: &str,
        id: ids::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        debug!(resource_type = %id.typ, "creating DNS resource");
        let dep_qid: ids::DeploymentQid = deployment_qid
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid deployment QID '{deployment_qid}': {e}"))?;
        self.dispatch(dep_qid.environment_qid(), &id, inputs).await
    }

    async fn update_resource(
        &mut self,
        deployment_qid: &str,
        id: ids::ResourceId,
        _prev_inputs: sclc::Record,
        _prev_outputs: sclc::Record,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        debug!(resource_type = %id.typ, "updating DNS resource");
        let dep_qid: ids::DeploymentQid = deployment_qid
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid deployment QID '{deployment_qid}': {e}"))?;
        self.dispatch(dep_qid.environment_qid(), &id, inputs).await
    }

    async fn delete_resource(
        &mut self,
        deployment_qid: &str,
        id: ids::ResourceId,
        inputs: sclc::Record,
        _outputs: sclc::Record,
    ) -> anyhow::Result<()> {
        debug!(resource_type = %id.typ, "deleting DNS resource");
        let dep_qid: ids::DeploymentQid = deployment_qid
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid deployment QID '{deployment_qid}': {e}"))?;
        let env_qid = dep_qid.environment_qid();
        match id.typ.as_str() {
            A_RECORD_RESOURCE_TYPE => {
                let name = match inputs.get("name") {
                    sclc::Value::Str(s) => s.clone(),
                    _ => anyhow::bail!("missing or invalid 'name' input"),
                };
                let fqdn = self.fqdn(&name, env_qid);
                self.inner.store.delete_a_record(&fqdn).await?;
                Ok(())
            }
            _ => anyhow::bail!("unsupported resource type: {}", id.typ),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();

    let store = dns_store::DnsStore::connect(&args.redis_hostname).await?;
    let plugin = DnsPlugin::new(store.clone(), args.zone.clone());

    let dns_addr: SocketAddr = format!("0.0.0.0:{}", args.dns_port).parse()?;

    tracing::info!("Starting RTP server on {}", args.bind);
    tracing::info!("Starting DNS server on {dns_addr}");

    tokio::select! {
        result = rtp::serve(&args.bind, move || plugin.clone()) => {
            error!("RTP server exited");
            result?;
        }
        result = dns_server::run(dns_addr, args.zone, store) => {
            error!("DNS server exited");
            result?;
        }
    }

    Ok(())
}

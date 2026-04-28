use std::collections::BTreeSet;

use anyhow::Context;
use clap::Parser;
use sclc::ValueAssertions;
use tracing::debug;

const GET_RESOURCE_TYPE: &str = "Std/HTTP.Get";
const SKYR_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser)]
struct Args {
    #[arg(long)]
    bind: String,
}

struct HttpPlugin {
    client: reqwest::Client,
}

impl HttpPlugin {
    fn new() -> Self {
        let client = reqwest::Client::builder()
            .build()
            .expect("failed to build HTTP client");
        Self { client }
    }

    async fn get_resource(
        &self,
        deployment_qid: &str,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        let url = inputs
            .get("url")
            .assert_str_ref()
            .context("missing or invalid 'url' input")?;

        let user_agent = format!("Skyr/{SKYR_VERSION} ({deployment_qid})");

        debug!(url, user_agent, "performing HTTP GET");
        let response = self
            .client
            .get(url)
            .header(reqwest::header::USER_AGENT, &user_agent)
            .send()
            .await
            .with_context(|| format!("HTTP GET failed for {url}"))?;
        let status = response.status().as_u16() as i64;
        debug!(url, status, "HTTP GET completed");

        let mut outputs = sclc::Record::default();
        outputs.insert(String::from("status"), sclc::Value::Int(status));
        Ok(sclc::Resource {
            inputs,
            outputs,
            dependencies: vec![],
            markers: BTreeSet::from([sclc::Marker::Volatile]),
        })
    }

    async fn dispatch(
        &self,
        deployment_qid: &str,
        id: &ids::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        match id.typ.as_str() {
            GET_RESOURCE_TYPE => self.get_resource(deployment_qid, inputs).await,
            _ => anyhow::bail!("unsupported resource type: {}", id.typ),
        }
    }
}

#[async_trait::async_trait]
impl rtp::Plugin for HttpPlugin {
    async fn create_resource(
        &mut self,
        deployment_qid: &str,
        id: ids::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        debug!(resource_type = %id.typ, "creating http resource");
        self.dispatch(deployment_qid, &id, inputs).await
    }

    async fn update_resource(
        &mut self,
        deployment_qid: &str,
        id: ids::ResourceId,
        _prev_inputs: sclc::Record,
        _prev_outputs: sclc::Record,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        debug!(resource_type = %id.typ, "updating http resource");
        self.dispatch(deployment_qid, &id, inputs).await
    }

    async fn check(
        &self,
        deployment_qid: &str,
        id: ids::ResourceId,
        resource: sclc::Resource,
    ) -> anyhow::Result<sclc::Resource> {
        debug!(resource_type = %id.typ, "checking http resource");
        match id.typ.as_str() {
            GET_RESOURCE_TYPE => self.get_resource(deployment_qid, resource.inputs).await,
            _ => Ok(resource),
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
    rtp::serve(&args.bind, HttpPlugin::new).await?;
    Ok(())
}

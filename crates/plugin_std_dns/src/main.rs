use clap::Parser;
use tracing::debug;

const A_RECORD_RESOURCE_TYPE: &str = "Std/DNS.ARecord";

#[derive(Parser)]
struct Args {
    #[arg(long)]
    bind: String,
}

struct DnsPlugin;

impl DnsPlugin {
    fn new() -> Self {
        Self
    }

    fn dispatch(
        &self,
        id: &ids::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        match id.typ.as_str() {
            A_RECORD_RESOURCE_TYPE => Ok(sclc::Resource {
                inputs,
                outputs: sclc::Record::default(),
                dependencies: vec![],
                markers: Default::default(),
            }),
            _ => anyhow::bail!("unsupported resource type: {}", id.typ),
        }
    }
}

#[async_trait::async_trait]
impl rtp::Plugin for DnsPlugin {
    async fn create_resource(
        &mut self,
        _environment_qid: &str,
        _deployment_id: &str,
        id: ids::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        debug!(resource_type = %id.typ, "creating DNS resource");
        self.dispatch(&id, inputs)
    }

    async fn update_resource(
        &mut self,
        _environment_qid: &str,
        _deployment_id: &str,
        id: ids::ResourceId,
        _prev_inputs: sclc::Record,
        _prev_outputs: sclc::Record,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        debug!(resource_type = %id.typ, "updating DNS resource");
        self.dispatch(&id, inputs)
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
    rtp::serve(&args.bind, DnsPlugin::new).await?;
    Ok(())
}

use anyhow::Context;
use clap::Parser;
use rand::{Rng, SeedableRng, rngs::StdRng};
use sclc::ValueAssertions;
use tracing::debug;

const INT_RESOURCE_TYPE: &str = "Std/Random.Int";

#[derive(Parser)]
struct Args {
    #[arg(long)]
    bind: String,
}

struct RandomPlugin {
    rng: StdRng,
}

impl RandomPlugin {
    fn new() -> Self {
        Self {
            rng: StdRng::from_os_rng(),
        }
    }

    fn dispatch(
        &mut self,
        id: &ids::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        match id.typ.as_str() {
            INT_RESOURCE_TYPE => self.gen_int_resource(inputs),
            _ => anyhow::bail!("unsupported resource type: {}", id.typ),
        }
    }

    fn gen_int_resource(&mut self, inputs: sclc::Record) -> anyhow::Result<sclc::Resource> {
        let min = *inputs
            .get("min")
            .assert_int_ref()
            .context("missing or invalid 'min' input")?;
        let max = *inputs
            .get("max")
            .assert_int_ref()
            .context("missing or invalid 'max' input")?;

        anyhow::ensure!(
            min <= max,
            "min ({min}) must not be greater than max ({max})"
        );

        debug!(min, max, "generating random integer");
        let result = self.rng.random_range(min..=max);
        debug!(result, "generated random integer");

        let mut outputs = sclc::Record::default();
        outputs.insert(String::from("result"), sclc::Value::Int(result));
        Ok(sclc::Resource {
            inputs,
            outputs,
            dependencies: vec![],
            markers: Default::default(),
        })
    }
}

#[async_trait::async_trait]
impl rtp::Plugin for RandomPlugin {
    async fn create_resource(
        &mut self,
        _deployment_qid: &str,
        id: ids::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        debug!(resource_type = %id.typ, "creating random resource");
        self.dispatch(&id, inputs)
    }

    async fn update_resource(
        &mut self,
        _deployment_qid: &str,
        id: ids::ResourceId,
        _prev_inputs: sclc::Record,
        _prev_outputs: sclc::Record,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        debug!(resource_type = %id.typ, "updating random resource");
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
    rtp::serve(&args.bind, RandomPlugin::new).await?;
    Ok(())
}

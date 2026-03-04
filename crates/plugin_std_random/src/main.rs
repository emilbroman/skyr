use clap::Parser;
use rand::{Rng, SeedableRng, rngs::StdRng};
use sclc::ValueAssertions;

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

    fn gen_int_resource(&mut self, inputs: sclc::Record) -> anyhow::Result<sclc::Resource> {
        let min = *inputs.get("min").assert_int_ref()?;
        let max = *inputs.get("max").assert_int_ref()?;
        let result = self.rng.random_range(min..=max);

        let mut outputs = sclc::Record::default();
        outputs.insert(String::from("result"), sclc::Value::Int(result));
        Ok(sclc::Resource {
            inputs,
            outputs,
            dependencies: vec![],
        })
    }
}

#[async_trait::async_trait]
impl rtp::Plugin for RandomPlugin {
    async fn create_resource(
        &mut self,
        id: sclc::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        match id.ty.as_str() {
            "Std/Random.Int" => self.gen_int_resource(inputs),
            _ => anyhow::bail!("unsupported resource type: {}", id.ty),
        }
    }

    async fn update_resource(
        &mut self,
        id: sclc::ResourceId,
        _prev_inputs: sclc::Record,
        _prev_outputs: sclc::Record,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        match id.ty.as_str() {
            "Std/Random.Int" => self.gen_int_resource(inputs),
            _ => anyhow::bail!("unsupported resource type: {}", id.ty),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    rtp::serve(&args.bind, RandomPlugin::new).await?;
    Ok(())
}

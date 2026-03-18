use clap::Parser;
use sclc::{Value, ValueAssertions};
use tracing::{error, info, warn};

const ARTIFACT_RESOURCE_TYPE: &str = "Std/Artifact.File";

#[derive(Parser)]
struct Args {
    #[arg(long)]
    bind: String,

    #[arg(long, default_value = "http://127.0.0.1:9000")]
    adb_endpoint_url: String,

    #[arg(long)]
    adb_presign_endpoint_url: Option<String>,

    #[arg(long, default_value = "skyr-artifacts")]
    adb_bucket: String,

    #[arg(long, default_value = "minioadmin")]
    adb_access_key_id: String,

    #[arg(long, default_value = "minioadmin")]
    adb_secret_access_key: String,

    #[arg(long, default_value = "us-east-1")]
    adb_region: String,
}

#[derive(Clone)]
struct ArtifactPlugin {
    adb: adb::Client,
}

impl ArtifactPlugin {
    fn new(adb: adb::Client) -> Self {
        Self { adb }
    }

    async fn materialize_artifact(
        &self,
        id: &ids::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        if id.typ != ARTIFACT_RESOURCE_TYPE {
            anyhow::bail!("unsupported resource type: {}", id.typ);
        }

        let name = inputs.get("name").assert_str_ref()?;
        let namespace = inputs.get("namespace").assert_str_ref()?;
        let contents = inputs.get("contents").assert_str_ref()?;
        let media_type = match inputs.get("mediaType") {
            Value::Str(value) => Some(value.as_str()),
            Value::Nil => None,
            other => {
                anyhow::bail!("invalid input for mediaType: expected Str? but got {other}");
            }
        };

        let body = adb::ArtifactBody::from(contents.as_bytes().to_vec());

        info!(
            resource_type = id.typ.as_str(),
            resource_name = id.name.as_str(),
            namespace = namespace,
            name = name,
            "materializing artifact resource"
        );

        let header = match self.adb.write(namespace, name, media_type, body).await {
            Ok(header) => header,
            Err(adb::WriteError::AlreadyExists { .. }) => {
                warn!(
                    resource_type = id.typ.as_str(),
                    resource_name = id.name.as_str(),
                    namespace = namespace,
                    name = name,
                    "artifact already exists, treating create as idempotent"
                );
                self.adb
                    .read_header(namespace, name)
                    .await?
                    .ok_or_else(|| {
                        anyhow::anyhow!("artifact already exists but could not be read")
                    })?
            }
            Err(error) => return Err(error.into()),
        };

        info!(
            resource_type = id.typ.as_str(),
            resource_name = id.name.as_str(),
            namespace = header.namespace.as_str(),
            name = header.name.as_str(),
            "artifact resource materialized"
        );

        let private_url = self.adb.private_read_url(&header.namespace, &header.name)?;

        let mut outputs = sclc::Record::default();
        outputs.insert(
            String::from("namespace"),
            sclc::Value::Str(header.namespace),
        );
        outputs.insert(String::from("name"), sclc::Value::Str(header.name));
        outputs.insert(
            String::from("media_type"),
            sclc::Value::Str(header.media_type),
        );
        outputs.insert(String::from("url"), sclc::Value::Str(private_url));

        Ok(sclc::Resource {
            inputs,
            outputs,
            dependencies: vec![],
            markers: Default::default(),
        })
    }

    async fn materialize_with_error_log(
        &self,
        id: &ids::ResourceId,
        inputs: sclc::Record,
        operation: &str,
    ) -> anyhow::Result<sclc::Resource> {
        let result = self.materialize_artifact(id, inputs).await;
        if let Err(err) = &result {
            error!(
                resource_type = id.typ.as_str(),
                resource_name = id.name.as_str(),
                err = %err,
                "artifact {operation} failed"
            );
        }
        result
    }
}

#[async_trait::async_trait]
impl rtp::Plugin for ArtifactPlugin {
    async fn create_resource(
        &mut self,
        _environment_qid: &str,
        _deployment_id: &str,
        id: ids::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        self.materialize_with_error_log(&id, inputs, "create_resource")
            .await
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
        self.materialize_with_error_log(&id, inputs, "update_resource")
            .await
    }

    async fn delete_resource(
        &mut self,
        _environment_qid: &str,
        _deployment_id: &str,
        id: ids::ResourceId,
        _inputs: sclc::Record,
        _outputs: sclc::Record,
    ) -> anyhow::Result<()> {
        if id.typ != ARTIFACT_RESOURCE_TYPE {
            anyhow::bail!("unsupported resource type: {}", id.typ);
        }

        // Artifacts are retained even when the owning deployment is torn down.
        info!(
            resource_type = id.typ.as_str(),
            resource_name = id.name.as_str(),
            "artifact delete is a no-op"
        );
        Ok(())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .compact()
        .init();

    let args = Args::parse();
    info!(
        bind = args.bind.as_str(),
        adb_endpoint_url = args.adb_endpoint_url.as_str(),
        adb_bucket = args.adb_bucket.as_str(),
        adb_region = args.adb_region.as_str(),
        "starting Std/Artifact plugin"
    );

    let mut adb_builder = adb::ClientBuilder::new()
        .bucket(args.adb_bucket)
        .endpoint_url(args.adb_endpoint_url)
        .region(args.adb_region)
        .access_key_id(args.adb_access_key_id)
        .secret_access_key(args.adb_secret_access_key)
        .create_bucket_if_missing(true);
    if let Some(adb_presign_endpoint_url) = args.adb_presign_endpoint_url {
        adb_builder = adb_builder.presign_endpoint_url(adb_presign_endpoint_url);
    }
    let adb = adb_builder.build().await?;

    rtp::serve(&args.bind, move || ArtifactPlugin::new(adb.clone())).await?;
    Ok(())
}

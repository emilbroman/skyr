use clap::Parser;
use sclc::{Value, ValueAssertions};
use tracing::{debug, error, info, warn};

const ARTIFACT_RESOURCE_TYPE: &str = "Std/Artifact.File";

/// Maximum artifact name length in bytes.
const MAX_ARTIFACT_NAME_LENGTH: usize = 512;

/// Default maximum artifact size in bytes (10 MiB).
const DEFAULT_MAX_ARTIFACT_SIZE_BYTES: usize = 10 * 1024 * 1024;

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

    #[arg(long, env = "ADB_ACCESS_KEY_ID", default_value = "minioadmin")]
    adb_access_key_id: String,

    #[arg(long, env = "ADB_SECRET_ACCESS_KEY", default_value = "minioadmin")]
    adb_secret_access_key: String,

    #[arg(long, default_value = "us-east-1")]
    adb_region: String,

    #[arg(long, default_value_t = DEFAULT_MAX_ARTIFACT_SIZE_BYTES)]
    max_artifact_size_bytes: usize,
}

/// Validated artifact inputs extracted from an SCL record.
struct ArtifactInputs<'a> {
    name: &'a str,
    namespace: &'a str,
    contents: &'a str,
    media_type: Option<&'a str>,
}

impl<'a> ArtifactInputs<'a> {
    /// Parse and validate artifact inputs from an SCL record.
    fn from_record(inputs: &'a sclc::Record, max_size_bytes: usize) -> anyhow::Result<Self> {
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

        // Validate artifact name.
        if name.is_empty() {
            anyhow::bail!("artifact name must not be empty");
        }
        if name.trim() != name {
            anyhow::bail!("artifact name must not have leading or trailing whitespace");
        }
        if name.len() > MAX_ARTIFACT_NAME_LENGTH {
            anyhow::bail!(
                "artifact name exceeds maximum length of {MAX_ARTIFACT_NAME_LENGTH} bytes"
            );
        }

        // Validate artifact size.
        if contents.len() > max_size_bytes {
            anyhow::bail!(
                "artifact contents size ({} bytes) exceeds maximum allowed size ({} bytes)",
                contents.len(),
                max_size_bytes,
            );
        }

        Ok(Self {
            name,
            namespace,
            contents,
            media_type,
        })
    }
}

fn assert_resource_type(id: &ids::ResourceId) -> anyhow::Result<()> {
    if id.typ != ARTIFACT_RESOURCE_TYPE {
        anyhow::bail!("unsupported resource type: {}", id.typ);
    }
    Ok(())
}

#[derive(Clone)]
struct ArtifactPlugin {
    adb: adb::Client,
    max_artifact_size_bytes: usize,
}

impl ArtifactPlugin {
    fn new(adb: adb::Client, max_artifact_size_bytes: usize) -> Self {
        Self {
            adb,
            max_artifact_size_bytes,
        }
    }

    async fn materialize_artifact(
        &self,
        id: &ids::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        assert_resource_type(id)?;

        let artifact = ArtifactInputs::from_record(&inputs, self.max_artifact_size_bytes)?;

        let body = adb::ArtifactBody::from(artifact.contents.as_bytes().to_vec());

        debug!(
            resource_type = id.typ.as_str(),
            resource_name = id.name.as_str(),
            namespace = artifact.namespace,
            "materializing artifact resource"
        );

        // If the artifact already exists (e.g. from a previous interrupted deployment),
        // treat it as a success and read back the existing header. This makes artifact
        // creation idempotent.
        let header = match self
            .adb
            .write(artifact.namespace, artifact.name, artifact.media_type, body)
            .await
        {
            Ok(header) => header,
            Err(adb::WriteError::AlreadyExists { .. }) => {
                warn!(
                    resource_type = id.typ.as_str(),
                    resource_name = id.name.as_str(),
                    "artifact already exists, treating create as idempotent"
                );
                self.adb
                    .read_header(artifact.namespace, artifact.name)
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
            String::from("mediaType"),
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
        assert_resource_type(&id)?;

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
        max_artifact_size_bytes = args.max_artifact_size_bytes,
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

    let max_size = args.max_artifact_size_bytes;
    rtp::serve(&args.bind, move || {
        ArtifactPlugin::new(adb.clone(), max_size)
    })
    .await?;
    Ok(())
}

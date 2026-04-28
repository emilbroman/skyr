use clap::Parser;
use ldb::Severity;
use std::collections::HashMap;
use std::str::FromStr;
use std::time::Instant;
use tokio::task;
use tracing::Instrument;

/// Maximum byte size for JSON input values before deserialization.
/// Prevents resource exhaustion from oversized messages.
const MAX_INPUT_SIZE_BYTES: usize = 1_048_576; // 1 MiB

/// Maximum byte size for plugin output values before persistence.
/// Prevents a compromised plugin from exhausting RDB storage.
const MAX_OUTPUT_SIZE_BYTES: usize = 1_048_576; // 1 MiB

#[derive(Parser)]
enum Program {
    Daemon {
        #[clap(long = "rdb-hostname", default_value = "localhost")]
        rdb_hostname: String,

        #[clap(long = "rtq-hostname", default_value = "localhost")]
        rtq_hostname: String,

        #[clap(long = "rq-hostname", default_value = "localhost")]
        rq_hostname: String,

        #[clap(long = "ldb-hostname", default_value = "localhost")]
        ldb_hostname: String,

        #[clap(long = "worker-index", default_value_t = 0)]
        worker_index: u16,

        #[clap(long = "worker-count", default_value_t = 1)]
        worker_count: u16,

        #[clap(long = "local-workers", default_value_t = 1)]
        local_workers: u16,

        #[clap(long = "plugin")]
        plugin: Vec<PluginSpec>,
    },
}

#[derive(Clone, Debug)]
struct PluginSpec {
    name: sclc::ModuleId,
    target: rtp::Target,
}

impl FromStr for PluginSpec {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (name_str, target_str) = s.split_once('@').ok_or_else(|| {
            format!(
                "invalid plugin spec `{s}`: expected NAME@TARGET (e.g. Std/Random@tcp://127.0.0.1:50051)"
            )
        })?;

        let name = parse_module_id(name_str)
            .ok_or_else(|| format!("invalid plugin name `{name_str}`: empty module path"))?;
        let target = target_str
            .parse::<rtp::Target>()
            .map_err(|error| format!("invalid plugin target `{target_str}`: {error}"))?;

        Ok(Self { name, target })
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

    match Program::parse() {
        Program::Daemon {
            rdb_hostname,
            rtq_hostname,
            rq_hostname,
            ldb_hostname,
            worker_index,
            worker_count,
            local_workers,
            plugin,
        } => {
            tracing::info!("starting resource transition engine daemon");

            if local_workers == 0 {
                anyhow::bail!("--local-workers must be at least 1");
            }
            if worker_count == 0 {
                anyhow::bail!("--worker-count must be at least 1");
            }
            if worker_index >= worker_count {
                anyhow::bail!("--worker-index must be less than --worker-count");
            }
            if worker_index.saturating_add(local_workers) > worker_count {
                anyhow::bail!("--worker-index + --local-workers must be <= --worker-count");
            }

            let uri = format!("amqp://{}:5672/%2f", rtq_hostname);
            let rq_uri = format!("amqp://{}:5672/%2f", rq_hostname);
            let rdb_client = rdb::ClientBuilder::new()
                .known_node(&rdb_hostname)
                .build()
                .await?;
            let ldb_publisher = ldb::ClientBuilder::new()
                .brokers(format!("{}:9092", ldb_hostname))
                .build_publisher()
                .await?;
            let rq_publisher = rq::ClientBuilder::new()
                .uri(rq_uri)
                .build_publisher()
                .await?;
            let plugins = dial_plugins(&plugin).await?;
            let mut handles = Vec::new();

            for offset in 0..local_workers {
                let index = worker_index + offset;
                let worker_cfg = rtq::WorkerConfig {
                    worker_index: index,
                    worker_count,
                };

                let consumer = rtq::ClientBuilder::new()
                    .uri(uri.clone())
                    .build_consumer(worker_cfg)
                    .await?;

                let span =
                    tracing::info_span!("worker", worker = %format!("{}/{}", index, worker_count));
                tracing::info!(
                    parent: &span,
                    shards = ?consumer.owned_shards(),
                    "started rtq consumer",
                );

                let ctx = WorkerContext {
                    rdb_client: rdb_client.clone(),
                    ldb_publisher: ldb_publisher.clone(),
                    rq_publisher: rq_publisher.clone(),
                    plugins: plugins.clone(),
                };
                handles.push(task::spawn(worker_loop(consumer, ctx).instrument(span)));
            }

            for handle in handles {
                handle.await?;
            }

            Ok(())
        }
    }
}

struct WorkerContext {
    rdb_client: rdb::Client,
    ldb_publisher: ldb::Publisher,
    rq_publisher: rq::Publisher,
    plugins: HashMap<sclc::ModuleId, rtp::PluginClient>,
}

async fn worker_loop(mut consumer: rtq::Consumer, ctx: WorkerContext) {
    loop {
        let keep_running = match worker_loop_iteration(&mut consumer, &ctx).await {
            Ok(keep_running) => keep_running,
            Err(error) => {
                tracing::error!(error = %error, "worker loop iteration failed");
                true
            }
        };

        if !keep_running {
            return;
        }
    }
}

async fn worker_loop_iteration(
    consumer: &mut rtq::Consumer,
    ctx: &WorkerContext,
) -> anyhow::Result<bool> {
    tracing::info!("polling rtq consumer for next message");
    let Some(delivery) = consumer.next().await? else {
        tracing::warn!("rtq consumer stream closed");
        return Ok(false);
    };
    let (message_type, resource) = message_meta(&delivery.message);
    tracing::info!(
        message_type,
        environment_qid = %resource.environment_qid,
        resource_type = %resource.resource_type(),
        resource_name = %resource.resource_name(),
        redelivered = delivery.redelivered(),
        "received rtq message",
    );

    match &delivery.message {
        rtq::Message::Create(message) => {
            handle_create(message, &delivery, ctx).await?;
        }
        rtq::Message::Destroy(message) => {
            handle_destroy(message, &delivery, ctx).await?;
        }
        rtq::Message::Adopt(message) => {
            handle_adopt(message, &delivery, ctx).await?;
        }
        rtq::Message::Restore(message) => {
            handle_restore(message, &delivery, ctx).await?;
        }
        rtq::Message::Check(message) => {
            handle_check(message, &delivery, ctx).await?;
        }
    }

    // Placeholder behavior until transition execution is implemented.
    delivery.ack().await?;
    tracing::info!(
        message_type,
        environment_qid = %resource.environment_qid,
        resource_type = %resource.resource_type(),
        resource_name = %resource.resource_name(),
        "acknowledged rtq message",
    );
    Ok(true)
}

fn deployment_qid(
    environment_qid: &ids::EnvironmentQid,
    deployment_id: &ids::DeploymentId,
) -> String {
    environment_qid
        .deployment(deployment_id.clone())
        .to_string()
}

/// Check that a JSON value's serialized size is within the given limit.
fn check_json_size(value: &serde_json::Value, limit: usize) -> bool {
    value.to_string().len() <= limit
}

fn resource_qid_string(resource: &rtq::ResourceRef) -> String {
    resource_qid_typed(resource).to_string()
}

fn resource_qid_typed(resource: &rtq::ResourceRef) -> ids::ResourceQid {
    ids::ResourceQid::new(
        resource.environment_qid.clone(),
        resource.resource_id.clone(),
    )
}

/// Parse a module ID string like "Std/Random" into a `ModuleId`.
/// The first segment becomes the package, the rest becomes the module path.
fn parse_module_id(s: &str) -> Option<sclc::ModuleId> {
    let segments: Vec<String> = s
        .split('/')
        .filter(|seg| !seg.is_empty())
        .map(str::to_owned)
        .collect();
    if segments.is_empty() {
        return None;
    }
    Some(sclc::ModuleId::new(
        sclc::PackageId::new(vec![segments[0].clone()]),
        segments[1..].to_vec(),
    ))
}

fn resolve_plugin(
    resource_type: &str,
    plugins: &HashMap<sclc::ModuleId, rtp::PluginClient>,
) -> Option<(sclc::ModuleId, rtp::PluginClient)> {
    let plugin_name = plugin_name_for_resource_type(resource_type)?;
    let module_id = parse_module_id(plugin_name)?;
    let client = plugins.get(&module_id)?;
    Some((module_id, client.clone()))
}

fn resource_id_from_ref(resource: &rtq::ResourceRef) -> ids::ResourceId {
    resource.resource_id.clone()
}

/// Persist resource state to RDB in a single helper, reducing duplication
/// across create/update handlers (1.2).
async fn persist_resource_state(
    resource_client: &rdb::ResourceClient,
    inputs: &sclc::Record,
    owner_qid: &str,
    outputs: &sclc::Record,
    markers: Option<&std::collections::BTreeSet<sclc::Marker>>,
    dependencies: &[ids::ResourceId],
    source_trace: &ids::SourceTrace,
) -> Result<(), anyhow::Error> {
    resource_client
        .set_input(inputs.clone(), owner_qid.to_string())
        .await?;
    resource_client.set_dependencies(dependencies).await?;
    resource_client.set_output(outputs.clone()).await?;
    if let Some(markers) = markers {
        resource_client.set_markers(markers).await?;
    }
    resource_client.set_source_trace(source_trace).await?;
    Ok(())
}

async fn handle_create(
    message: &rtq::CreateMessage,
    delivery: &rtq::Delivery,
    ctx: &WorkerContext,
) -> anyhow::Result<()> {
    let start = Instant::now();
    let res_qid_typed = resource_qid_typed(&message.resource);
    let resource_client = ctx
        .rdb_client
        .namespace(message.resource.environment_qid.to_string())
        .resource(
            message.resource.resource_type().to_owned(),
            message.resource.resource_name().to_owned(),
        );
    match resource_client.get().await {
        Ok(Some(_)) => {
            // Idempotent drop — no transition was processed, no report.
            tracing::info!(
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type(),
                resource_name = %message.resource.resource_name(),
                "dropping idempotent create for existing resource",
            );
            delivery.ack().await?;
            return Ok(());
        }
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type(),
                resource_name = %message.resource.resource_name(),
                error = %error,
                "failed to read resource state before create",
            );
            publish_report(
                &ctx.rq_publisher,
                res_qid_typed,
                start.elapsed().as_millis() as u64,
                outcome_failure(rq::IncidentCategory::SystemError, error),
                rq::ResourceOperationalState::Pending,
                false,
                false,
            )
            .await;
            delivery.nack(false).await?;
            return Ok(());
        }
    }

    let Some((plugin_name, mut plugin)) =
        resolve_plugin(message.resource.resource_type(), &ctx.plugins)
    else {
        tracing::warn!(
            environment_qid = %message.resource.environment_qid,
            resource_type = %message.resource.resource_type(),
            "no plugin available for resource type on create",
        );
        publish_report(
            &ctx.rq_publisher,
            res_qid_typed,
            start.elapsed().as_millis() as u64,
            outcome_failure(
                rq::IncidentCategory::SystemError,
                format!(
                    "no plugin available for resource type {}",
                    message.resource.resource_type()
                ),
            ),
            rq::ResourceOperationalState::Pending,
            false,
            false,
        )
        .await;
        delivery.nack(false).await?;
        return Ok(());
    };

    // Validate input size before deserialization (3.4).
    if !check_json_size(&message.inputs, MAX_INPUT_SIZE_BYTES) {
        tracing::warn!(
            environment_qid = %message.resource.environment_qid,
            resource_type = %message.resource.resource_type(),
            resource_name = %message.resource.resource_name(),
            "create inputs exceed size limit",
        );
        publish_report(
            &ctx.rq_publisher,
            res_qid_typed,
            start.elapsed().as_millis() as u64,
            outcome_failure(
                rq::IncidentCategory::BadConfiguration,
                "create inputs exceed size limit",
            ),
            rq::ResourceOperationalState::Pending,
            false,
            false,
        )
        .await;
        delivery.nack(false).await?;
        return Ok(());
    }

    let inputs: sclc::Record = match serde_json::from_value(message.inputs.clone()) {
        Ok(inputs) => inputs,
        Err(error) => {
            tracing::warn!(
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type(),
                resource_name = %message.resource.resource_name(),
                error = %error,
                "invalid create inputs json",
            );
            publish_report(
                &ctx.rq_publisher,
                res_qid_typed,
                start.elapsed().as_millis() as u64,
                outcome_failure(rq::IncidentCategory::BadConfiguration, error),
                rq::ResourceOperationalState::Pending,
                false,
                false,
            )
            .await;
            delivery.nack(false).await?;
            return Ok(());
        }
    };

    let id = resource_id_from_ref(&message.resource);
    let dep_qid = deployment_qid(&message.resource.environment_qid, &message.deployment_id);
    let res_qid = resource_qid_string(&message.resource);
    let dep_short = message.deployment_id.short();

    tracing::info!(
        plugin = %plugin_name,
        environment_qid = %message.resource.environment_qid,
        resource_type = %message.resource.resource_type(),
        resource_name = %message.resource.resource_name(),
        "calling plugin create_resource",
    );
    log_event(
        &ctx.ldb_publisher,
        &dep_qid,
        &res_qid,
        Severity::Info,
        format!("{}: Creating {}", dep_short, id),
        format!("{}: Creating", dep_short),
    )
    .await;

    let resource = match plugin.create_resource(&dep_qid, id, inputs).await {
        Ok(resource) => resource,
        Err(error) => {
            tracing::warn!(
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type(),
                resource_name = %message.resource.resource_name(),
                error = %error,
                "create_resource plugin call failed",
            );
            log_event(
                &ctx.ldb_publisher,
                &dep_qid,
                &res_qid,
                Severity::Error,
                format!(
                    "{}: Failed to create {}: {}",
                    dep_short, message.resource.resource_id, error,
                ),
                format!("{}: Failed to create: {}", dep_short, error),
            )
            .await;
            publish_report(
                &ctx.rq_publisher,
                res_qid_typed,
                start.elapsed().as_millis() as u64,
                outcome_failure(rq::IncidentCategory::Crash, error),
                rq::ResourceOperationalState::Pending,
                false,
                false,
            )
            .await;
            delivery.nack(false).await?;
            return Ok(());
        }
    };

    let volatile = is_volatile_markers(&resource.markers);

    // Validate plugin output size before persistence (3.3).
    if let Ok(output_json) = serde_json::to_value(&resource.outputs)
        && !check_json_size(&output_json, MAX_OUTPUT_SIZE_BYTES)
    {
        tracing::warn!(
            environment_qid = %message.resource.environment_qid,
            resource_type = %message.resource.resource_type(),
            resource_name = %message.resource.resource_name(),
            "plugin create output exceeds size limit",
        );
        publish_report(
            &ctx.rq_publisher,
            res_qid_typed,
            start.elapsed().as_millis() as u64,
            outcome_failure(
                rq::IncidentCategory::SystemError,
                "plugin create output exceeds size limit",
            ),
            rq::ResourceOperationalState::Live,
            false,
            volatile,
        )
        .await;
        delivery.nack(false).await?;
        return Ok(());
    }

    if let Err(error) = persist_resource_state(
        &resource_client,
        &resource.inputs,
        &dep_qid,
        &resource.outputs,
        Some(&resource.markers),
        &dependencies_from_refs(&message.dependencies),
        &message.source_trace,
    )
    .await
    {
        tracing::warn!(
            environment_qid = %message.resource.environment_qid,
            resource_type = %message.resource.resource_type(),
            resource_name = %message.resource.resource_name(),
            error = %error,
            "failed to persist created resource state",
        );
        log_event(
            &ctx.ldb_publisher,
            &dep_qid,
            &res_qid,
            Severity::Error,
            format!(
                "{}: Failed to create {}: {}",
                dep_short, message.resource.resource_id, error,
            ),
            format!("{}: Failed to create: {}", dep_short, error),
        )
        .await;
        publish_report(
            &ctx.rq_publisher,
            res_qid_typed,
            start.elapsed().as_millis() as u64,
            outcome_failure(rq::IncidentCategory::SystemError, error),
            rq::ResourceOperationalState::Live,
            false,
            volatile,
        )
        .await;
        delivery.nack(false).await?;
        return Ok(());
    }

    tracing::info!(
        environment_qid = %message.resource.environment_qid,
        resource_type = %message.resource.resource_type(),
        resource_name = %message.resource.resource_name(),
        "created resource",
    );
    log_event(
        &ctx.ldb_publisher,
        &dep_qid,
        &res_qid,
        Severity::Info,
        format!(
            "{}: Created {}",
            dep_short,
            message.resource.resource_id.clone(),
        ),
        format!("{}: Created", dep_short),
    )
    .await;
    publish_report(
        &ctx.rq_publisher,
        res_qid_typed,
        start.elapsed().as_millis() as u64,
        outcome_success(),
        rq::ResourceOperationalState::Live,
        false,
        volatile,
    )
    .await;
    Ok(())
}

async fn handle_destroy(
    message: &rtq::DestroyMessage,
    delivery: &rtq::Delivery,
    ctx: &WorkerContext,
) -> anyhow::Result<()> {
    let start = Instant::now();
    let res_qid_typed = resource_qid_typed(&message.resource);
    let resource_client = ctx
        .rdb_client
        .namespace(message.resource.environment_qid.to_string())
        .resource(
            message.resource.resource_type().to_owned(),
            message.resource.resource_name().to_owned(),
        );
    let dep_qid = deployment_qid(&message.resource.environment_qid, &message.deployment_id);

    let current = match resource_client.get().await {
        Ok(Some(resource)) => {
            if resource.owner.as_deref() != Some(dep_qid.as_str()) {
                // Idempotent drop — different deployment owns this resource.
                tracing::info!(
                    environment_qid = %message.resource.environment_qid,
                    resource_type = %message.resource.resource_type(),
                    resource_name = %message.resource.resource_name(),
                    message_owner = %dep_qid,
                    current_owner = ?resource.owner,
                    "dropping delete for non-matching owner",
                );
                delivery.ack().await?;
                return Ok(());
            }
            resource
        }
        Ok(None) => {
            // Idempotent drop — resource already gone; some prior delivery
            // already reported its terminal state.
            tracing::info!(
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type(),
                resource_name = %message.resource.resource_name(),
                "dropping idempotent delete for missing resource",
            );
            delivery.ack().await?;
            return Ok(());
        }
        Err(error) => {
            tracing::warn!(
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type(),
                resource_name = %message.resource.resource_name(),
                error = %error,
                "failed to read resource state before delete",
            );
            publish_report(
                &ctx.rq_publisher,
                res_qid_typed,
                start.elapsed().as_millis() as u64,
                outcome_failure(rq::IncidentCategory::SystemError, error),
                rq::ResourceOperationalState::Live,
                false,
                false,
            )
            .await;
            delivery.nack(false).await?;
            return Ok(());
        }
    };

    let volatile = is_volatile_markers(&current.markers);

    let inputs = match current.inputs {
        Some(inputs) => inputs,
        None => {
            tracing::warn!(
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type(),
                resource_name = %message.resource.resource_name(),
                "missing current inputs for delete",
            );
            publish_report(
                &ctx.rq_publisher,
                res_qid_typed,
                start.elapsed().as_millis() as u64,
                outcome_failure(
                    rq::IncidentCategory::InconsistentState,
                    "missing current inputs for delete",
                ),
                rq::ResourceOperationalState::Live,
                false,
                volatile,
            )
            .await;
            delivery.nack(false).await?;
            return Ok(());
        }
    };
    let outputs = match current.outputs {
        Some(outputs) => outputs,
        None => {
            tracing::warn!(
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type(),
                resource_name = %message.resource.resource_name(),
                "missing current outputs for delete",
            );
            publish_report(
                &ctx.rq_publisher,
                res_qid_typed,
                start.elapsed().as_millis() as u64,
                outcome_failure(
                    rq::IncidentCategory::InconsistentState,
                    "missing current outputs for delete",
                ),
                rq::ResourceOperationalState::Live,
                false,
                volatile,
            )
            .await;
            delivery.nack(false).await?;
            return Ok(());
        }
    };

    let Some((plugin_name, mut plugin)) =
        resolve_plugin(message.resource.resource_type(), &ctx.plugins)
    else {
        tracing::warn!(
            environment_qid = %message.resource.environment_qid,
            resource_type = %message.resource.resource_type(),
            "no plugin available for resource type on delete",
        );
        publish_report(
            &ctx.rq_publisher,
            res_qid_typed,
            start.elapsed().as_millis() as u64,
            outcome_failure(
                rq::IncidentCategory::SystemError,
                format!(
                    "no plugin available for resource type {}",
                    message.resource.resource_type()
                ),
            ),
            rq::ResourceOperationalState::Live,
            false,
            volatile,
        )
        .await;
        delivery.nack(false).await?;
        return Ok(());
    };

    let id = resource_id_from_ref(&message.resource);
    let res_qid = resource_qid_string(&message.resource);
    let dep_short = message.deployment_id.short();

    tracing::info!(
        plugin = %plugin_name,
        environment_qid = %message.resource.environment_qid,
        resource_type = %message.resource.resource_type(),
        resource_name = %message.resource.resource_name(),
        "calling plugin delete_resource",
    );
    log_event(
        &ctx.ldb_publisher,
        &dep_qid,
        &res_qid,
        Severity::Info,
        format!("{}: Destroying {}", dep_short, id),
        format!("{}: Destroying", dep_short),
    )
    .await;

    if let Err(error) = plugin.delete_resource(&dep_qid, id, inputs, outputs).await {
        tracing::warn!(
            environment_qid = %message.resource.environment_qid,
            resource_type = %message.resource.resource_type(),
            resource_name = %message.resource.resource_name(),
            error = %error,
            "delete_resource plugin call failed",
        );
        log_event(
            &ctx.ldb_publisher,
            &dep_qid,
            &res_qid,
            Severity::Error,
            format!(
                "{}: Failed to destroy {}: {}",
                dep_short, message.resource.resource_id, error,
            ),
            format!("{}: Failed to destroy: {}", dep_short, error),
        )
        .await;
        publish_report(
            &ctx.rq_publisher,
            res_qid_typed,
            start.elapsed().as_millis() as u64,
            outcome_failure(rq::IncidentCategory::Crash, error),
            rq::ResourceOperationalState::Live,
            false,
            volatile,
        )
        .await;
        delivery.nack(false).await?;
        return Ok(());
    }

    if let Err(error) = resource_client.delete().await {
        tracing::warn!(
            environment_qid = %message.resource.environment_qid,
            resource_type = %message.resource.resource_type(),
            resource_name = %message.resource.resource_name(),
            error = %error,
            "failed to delete resource from rdb",
        );
        log_event(
            &ctx.ldb_publisher,
            &dep_qid,
            &res_qid,
            Severity::Error,
            format!(
                "{}: Failed to destroy {}: {}",
                dep_short, message.resource.resource_id, error,
            ),
            format!("{}: Failed to destroy: {}", dep_short, error),
        )
        .await;
        // The plugin reported success but RDB cleanup failed; the provider-side
        // resource is gone, so we report Destroyed but classify it as a Skyr
        // infrastructure problem (not terminal, since the next delivery will
        // hit the idempotent-missing path or retry).
        publish_report(
            &ctx.rq_publisher,
            res_qid_typed,
            start.elapsed().as_millis() as u64,
            outcome_failure(rq::IncidentCategory::SystemError, error),
            rq::ResourceOperationalState::Destroyed,
            false,
            volatile,
        )
        .await;
        delivery.nack(false).await?;
        return Ok(());
    }

    tracing::info!(
        environment_qid = %message.resource.environment_qid,
        resource_type = %message.resource.resource_type(),
        resource_name = %message.resource.resource_name(),
        "deleted resource",
    );
    log_event(
        &ctx.ldb_publisher,
        &dep_qid,
        &res_qid,
        Severity::Info,
        format!(
            "{}: Destroyed {}",
            dep_short,
            message.resource.resource_id.clone(),
        ),
        format!("{}: Destroyed", dep_short),
    )
    .await;
    // Successful destroy is the terminal report for this resource.
    publish_report(
        &ctx.rq_publisher,
        res_qid_typed,
        start.elapsed().as_millis() as u64,
        outcome_success(),
        rq::ResourceOperationalState::Destroyed,
        true,
        volatile,
    )
    .await;
    Ok(())
}

struct UpdateParams<'a> {
    resource: &'a rtq::ResourceRef,
    owner_deployment_qid: &'a str,
    target_deployment_id: &'a ids::DeploymentId,
    desired_inputs: serde_json::Value,
    dependencies: &'a [rtq::ResourceRef],
    operation: &'a str,
    source_trace: &'a ids::SourceTrace,
}

/// Successful outcome of [`handle_update_inputs`]: the target deployment QID
/// the resource is now owned by, plus the post-operation volatility marker
/// the caller threads into its terminal `publish_report`.
struct UpdateInputsSuccess {
    target_dep_qid: String,
    volatile: bool,
}

async fn handle_update_inputs(
    params: UpdateParams<'_>,
    delivery: &rtq::Delivery,
    ctx: &WorkerContext,
    start: Instant,
) -> anyhow::Result<Option<UpdateInputsSuccess>> {
    let resource = params.resource;
    let owner_deployment_qid = params.owner_deployment_qid;
    let target_deployment_id = params.target_deployment_id;
    let desired_inputs = params.desired_inputs;
    let dependencies = params.dependencies;
    let operation = params.operation;
    let res_qid_typed = resource_qid_typed(resource);
    let resource_client = ctx
        .rdb_client
        .namespace(resource.environment_qid.to_string())
        .resource(
            resource.resource_type().to_owned(),
            resource.resource_name().to_owned(),
        );
    let current = match resource_client.get().await {
        Ok(Some(r)) => r,
        Ok(None) => {
            // Idempotent drop — no resource to act on.
            tracing::info!(
                environment_qid = %resource.environment_qid,
                resource_type = %resource.resource_type(),
                resource_name = %resource.resource_name(),
                operation,
                "dropping operation for missing resource",
            );
            delivery.ack().await?;
            return Ok(None);
        }
        Err(error) => {
            tracing::warn!(
                environment_qid = %resource.environment_qid,
                resource_type = %resource.resource_type(),
                resource_name = %resource.resource_name(),
                operation,
                error = %error,
                "failed to read resource state before operation",
            );
            publish_report(
                &ctx.rq_publisher,
                res_qid_typed,
                start.elapsed().as_millis() as u64,
                outcome_failure(rq::IncidentCategory::SystemError, error),
                rq::ResourceOperationalState::Live,
                false,
                false,
            )
            .await;
            delivery.nack(false).await?;
            return Ok(None);
        }
    };

    let mut volatile = is_volatile_markers(&current.markers);

    if current.owner.as_deref() != Some(owner_deployment_qid) {
        // Idempotent drop — different deployment owns this resource.
        tracing::info!(
            environment_qid = %resource.environment_qid,
            resource_type = %resource.resource_type(),
            resource_name = %resource.resource_name(),
            operation,
            message_owner = %owner_deployment_qid,
            current_owner = ?current.owner,
            "dropping operation for non-matching owner",
        );
        delivery.ack().await?;
        return Ok(None);
    }

    let prev_inputs = match current.inputs {
        Some(inputs) => inputs,
        None => {
            tracing::warn!(
                environment_qid = %resource.environment_qid,
                resource_type = %resource.resource_type(),
                resource_name = %resource.resource_name(),
                operation,
                "missing current inputs for operation",
            );
            publish_report(
                &ctx.rq_publisher,
                res_qid_typed,
                start.elapsed().as_millis() as u64,
                outcome_failure(
                    rq::IncidentCategory::InconsistentState,
                    "missing current inputs for operation",
                ),
                rq::ResourceOperationalState::Live,
                false,
                volatile,
            )
            .await;
            delivery.nack(false).await?;
            return Ok(None);
        }
    };
    let prev_outputs = match current.outputs.clone() {
        Some(outputs) => outputs,
        None => {
            tracing::warn!(
                environment_qid = %resource.environment_qid,
                resource_type = %resource.resource_type(),
                resource_name = %resource.resource_name(),
                operation,
                "missing current outputs for operation",
            );
            publish_report(
                &ctx.rq_publisher,
                res_qid_typed,
                start.elapsed().as_millis() as u64,
                outcome_failure(
                    rq::IncidentCategory::InconsistentState,
                    "missing current outputs for operation",
                ),
                rq::ResourceOperationalState::Live,
                false,
                volatile,
            )
            .await;
            delivery.nack(false).await?;
            return Ok(None);
        }
    };

    // Validate input size before deserialization (3.4).
    if !check_json_size(&desired_inputs, MAX_INPUT_SIZE_BYTES) {
        tracing::warn!(
            environment_qid = %resource.environment_qid,
            resource_type = %resource.resource_type(),
            resource_name = %resource.resource_name(),
            operation,
            "desired inputs exceed size limit",
        );
        publish_report(
            &ctx.rq_publisher,
            res_qid_typed,
            start.elapsed().as_millis() as u64,
            outcome_failure(
                rq::IncidentCategory::BadConfiguration,
                "desired inputs exceed size limit",
            ),
            rq::ResourceOperationalState::Live,
            false,
            volatile,
        )
        .await;
        delivery.nack(false).await?;
        return Ok(None);
    }

    let desired_inputs: sclc::Record = match serde_json::from_value(desired_inputs) {
        Ok(inputs) => inputs,
        Err(error) => {
            tracing::warn!(
                environment_qid = %resource.environment_qid,
                resource_type = %resource.resource_type(),
                resource_name = %resource.resource_name(),
                operation,
                error = %error,
                "invalid desired_inputs json",
            );
            publish_report(
                &ctx.rq_publisher,
                res_qid_typed,
                start.elapsed().as_millis() as u64,
                outcome_failure(rq::IncidentCategory::BadConfiguration, error),
                rq::ResourceOperationalState::Live,
                false,
                volatile,
            )
            .await;
            delivery.nack(false).await?;
            return Ok(None);
        }
    };

    let res_qid = resource_qid_string(resource);
    let dep_short = target_deployment_id.short();

    let mut inputs_to_persist = prev_inputs.clone();
    let mut outputs_to_persist = current.outputs.clone();
    let mut markers_to_persist = None;

    if prev_inputs != desired_inputs {
        let id = resource_id_from_ref(resource);
        let Some((plugin_name, mut plugin)) =
            resolve_plugin(resource.resource_type(), &ctx.plugins)
        else {
            tracing::warn!(
                environment_qid = %resource.environment_qid,
                resource_type = %resource.resource_type(),
                operation,
                "no plugin available for resource type",
            );
            publish_report(
                &ctx.rq_publisher,
                res_qid_typed,
                start.elapsed().as_millis() as u64,
                outcome_failure(
                    rq::IncidentCategory::SystemError,
                    format!(
                        "no plugin available for resource type {}",
                        resource.resource_type()
                    ),
                ),
                rq::ResourceOperationalState::Live,
                false,
                volatile,
            )
            .await;
            delivery.nack(false).await?;
            return Ok(None);
        };

        let target_dep_qid_for_log =
            deployment_qid(&resource.environment_qid, target_deployment_id);
        let verb = match operation {
            "adopt" => "Adopting",
            "restore" => "Restoring",
            _ => "Updating",
        };
        tracing::info!(
            plugin = %plugin_name,
            environment_qid = %resource.environment_qid,
            resource_type = %resource.resource_type(),
            resource_name = %resource.resource_name(),
            operation,
            "calling plugin update_resource",
        );
        log_event(
            &ctx.ldb_publisher,
            &target_dep_qid_for_log,
            &res_qid,
            Severity::Info,
            format!("{}: {} {}", dep_short, verb, id),
            format!("{}: {}", dep_short, verb),
        )
        .await;

        let updated = match plugin
            .update_resource(
                &target_dep_qid_for_log,
                id,
                prev_inputs,
                prev_outputs,
                desired_inputs,
            )
            .await
        {
            Ok(r) => r,
            Err(error) => {
                tracing::warn!(
                    environment_qid = %resource.environment_qid,
                    resource_type = %resource.resource_type(),
                    resource_name = %resource.resource_name(),
                    operation,
                    error = %error,
                    "update_resource plugin call failed",
                );
                log_event(
                    &ctx.ldb_publisher,
                    &target_dep_qid_for_log,
                    &res_qid,
                    Severity::Error,
                    format!(
                        "{}: Failed to update {}: {}",
                        dep_short, resource.resource_id, error,
                    ),
                    format!("{}: Failed to update: {}", dep_short, error),
                )
                .await;
                publish_report(
                    &ctx.rq_publisher,
                    res_qid_typed,
                    start.elapsed().as_millis() as u64,
                    outcome_failure(rq::IncidentCategory::Crash, error),
                    rq::ResourceOperationalState::Live,
                    false,
                    volatile,
                )
                .await;
                delivery.nack(false).await?;
                return Ok(None);
            }
        };

        volatile = is_volatile_markers(&updated.markers);

        // Validate plugin output size before persistence (3.3).
        if let Ok(output_json) = serde_json::to_value(&updated.outputs)
            && !check_json_size(&output_json, MAX_OUTPUT_SIZE_BYTES)
        {
            tracing::warn!(
                environment_qid = %resource.environment_qid,
                resource_type = %resource.resource_type(),
                resource_name = %resource.resource_name(),
                operation,
                "plugin update output exceeds size limit",
            );
            publish_report(
                &ctx.rq_publisher,
                res_qid_typed,
                start.elapsed().as_millis() as u64,
                outcome_failure(
                    rq::IncidentCategory::SystemError,
                    "plugin update output exceeds size limit",
                ),
                rq::ResourceOperationalState::Live,
                false,
                volatile,
            )
            .await;
            delivery.nack(false).await?;
            return Ok(None);
        }

        inputs_to_persist = updated.inputs;
        outputs_to_persist = Some(updated.outputs);
        markers_to_persist = Some(updated.markers);
    } else {
        tracing::info!(
            environment_qid = %resource.environment_qid,
            resource_type = %resource.resource_type(),
            resource_name = %resource.resource_name(),
            operation,
            "skipping plugin update_resource with unchanged inputs",
        );
    }

    let target_dep_qid = deployment_qid(&resource.environment_qid, target_deployment_id);

    let outputs = outputs_to_persist.unwrap_or_default();
    if let Err(error) = persist_resource_state(
        &resource_client,
        &inputs_to_persist,
        &target_dep_qid,
        &outputs,
        markers_to_persist.as_ref(),
        &dependencies_from_refs(dependencies),
        params.source_trace,
    )
    .await
    {
        tracing::warn!(
            environment_qid = %resource.environment_qid,
            resource_type = %resource.resource_type(),
            resource_name = %resource.resource_name(),
            operation,
            error = %error,
            "failed to persist resource state",
        );
        log_event(
            &ctx.ldb_publisher,
            &target_dep_qid,
            &res_qid,
            Severity::Error,
            format!(
                "{}: Failed to update {}: {}",
                dep_short, resource.resource_id, error,
            ),
            format!("{}: Failed to update: {}", dep_short, error),
        )
        .await;
        publish_report(
            &ctx.rq_publisher,
            res_qid_typed,
            start.elapsed().as_millis() as u64,
            outcome_failure(rq::IncidentCategory::SystemError, error),
            rq::ResourceOperationalState::Live,
            false,
            volatile,
        )
        .await;
        delivery.nack(false).await?;
        return Ok(None);
    }

    Ok(Some(UpdateInputsSuccess {
        target_dep_qid,
        volatile,
    }))
}

async fn handle_adopt(
    message: &rtq::AdoptMessage,
    delivery: &rtq::Delivery,
    ctx: &WorkerContext,
) -> anyhow::Result<()> {
    let start = Instant::now();
    let res_qid_typed = resource_qid_typed(&message.resource);
    let from_dep_qid = deployment_qid(
        &message.resource.environment_qid,
        &message.from_deployment_id,
    );
    let Some(success) = handle_update_inputs(
        UpdateParams {
            resource: &message.resource,
            owner_deployment_qid: &from_dep_qid,
            target_deployment_id: &message.to_deployment_id,
            desired_inputs: message.desired_inputs.clone(),
            dependencies: &message.dependencies,
            operation: "adopt",
            source_trace: &message.source_trace,
        },
        delivery,
        ctx,
        start,
    )
    .await?
    else {
        return Ok(());
    };
    let UpdateInputsSuccess {
        target_dep_qid: to_dep_qid,
        volatile,
    } = success;

    let res_qid = resource_qid_string(&message.resource);
    let resource_id = message.resource.resource_id.clone();
    let to_short = message.to_deployment_id.short();
    let from_short = message.from_deployment_id.short();

    tracing::info!(
        environment_qid = %message.resource.environment_qid,
        resource_type = %message.resource.resource_type(),
        resource_name = %message.resource.resource_name(),
        from_owner = %from_dep_qid,
        to_owner = %to_dep_qid,
        "adopted resource",
    );
    log_event(
        &ctx.ldb_publisher,
        &from_dep_qid,
        &res_qid,
        Severity::Info,
        format!("Relinquished {} to {}", resource_id, to_short),
        format!("Relinquished to {}", to_short),
    )
    .await;
    log_event(
        &ctx.ldb_publisher,
        &to_dep_qid,
        &res_qid,
        Severity::Info,
        format!("Adopted {} from {}", resource_id, from_short),
        format!("Adopted from {}", from_short),
    )
    .await;
    publish_report(
        &ctx.rq_publisher,
        res_qid_typed,
        start.elapsed().as_millis() as u64,
        outcome_success(),
        rq::ResourceOperationalState::Live,
        false,
        volatile,
    )
    .await;
    Ok(())
}

async fn handle_restore(
    message: &rtq::RestoreMessage,
    delivery: &rtq::Delivery,
    ctx: &WorkerContext,
) -> anyhow::Result<()> {
    let start = Instant::now();
    let res_qid_typed = resource_qid_typed(&message.resource);
    let dep_qid = deployment_qid(&message.resource.environment_qid, &message.deployment_id);
    let Some(success) = handle_update_inputs(
        UpdateParams {
            resource: &message.resource,
            owner_deployment_qid: &dep_qid,
            target_deployment_id: &message.deployment_id,
            desired_inputs: message.desired_inputs.clone(),
            dependencies: &message.dependencies,
            operation: "restore",
            source_trace: &message.source_trace,
        },
        delivery,
        ctx,
        start,
    )
    .await?
    else {
        return Ok(());
    };
    let UpdateInputsSuccess {
        target_dep_qid: dep_qid,
        volatile,
    } = success;

    let res_qid = resource_qid_string(&message.resource);
    let dep_short = message.deployment_id.short();

    tracing::info!(
        environment_qid = %message.resource.environment_qid,
        resource_type = %message.resource.resource_type(),
        resource_name = %message.resource.resource_name(),
        owner = %dep_qid,
        "restored resource",
    );
    log_event(
        &ctx.ldb_publisher,
        &dep_qid,
        &res_qid,
        Severity::Info,
        format!(
            "{}: Restored {}",
            dep_short,
            message.resource.resource_id.clone(),
        ),
        format!("{}: Restored", dep_short),
    )
    .await;
    publish_report(
        &ctx.rq_publisher,
        res_qid_typed,
        start.elapsed().as_millis() as u64,
        outcome_success(),
        rq::ResourceOperationalState::Live,
        false,
        volatile,
    )
    .await;
    Ok(())
}

async fn handle_check(
    message: &rtq::CheckMessage,
    delivery: &rtq::Delivery,
    ctx: &WorkerContext,
) -> anyhow::Result<()> {
    let start = Instant::now();
    let res_qid_typed = resource_qid_typed(&message.resource);
    let resource_client = ctx
        .rdb_client
        .namespace(message.resource.environment_qid.to_string())
        .resource(
            message.resource.resource_type().to_owned(),
            message.resource.resource_name().to_owned(),
        );
    let dep_qid = deployment_qid(&message.resource.environment_qid, &message.deployment_id);

    let current = match resource_client.get().await {
        Ok(Some(resource)) => {
            if resource.owner.as_deref() != Some(dep_qid.as_str()) {
                // Idempotent drop — different deployment owns this resource.
                tracing::info!(
                    environment_qid = %message.resource.environment_qid,
                    resource_type = %message.resource.resource_type(),
                    resource_name = %message.resource.resource_name(),
                    message_owner = %dep_qid,
                    current_owner = ?resource.owner,
                    "dropping check for non-matching owner",
                );
                delivery.ack().await?;
                return Ok(());
            }
            resource
        }
        Ok(None) => {
            // Idempotent drop — resource missing; some prior delivery already
            // reported its terminal state.
            tracing::info!(
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type(),
                resource_name = %message.resource.resource_name(),
                "dropping check for missing resource",
            );
            delivery.ack().await?;
            return Ok(());
        }
        Err(error) => {
            tracing::warn!(
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type(),
                resource_name = %message.resource.resource_name(),
                error = %error,
                "failed to read resource state before check",
            );
            publish_report(
                &ctx.rq_publisher,
                res_qid_typed,
                start.elapsed().as_millis() as u64,
                outcome_failure(rq::IncidentCategory::SystemError, error),
                rq::ResourceOperationalState::Live,
                false,
                false,
            )
            .await;
            delivery.nack(false).await?;
            return Ok(());
        }
    };

    let mut volatile = is_volatile_markers(&current.markers);

    let inputs = match current.inputs {
        Some(inputs) => inputs,
        None => {
            tracing::warn!(
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type(),
                resource_name = %message.resource.resource_name(),
                "missing current inputs for check",
            );
            publish_report(
                &ctx.rq_publisher,
                res_qid_typed,
                start.elapsed().as_millis() as u64,
                outcome_failure(
                    rq::IncidentCategory::InconsistentState,
                    "missing current inputs for check",
                ),
                rq::ResourceOperationalState::Live,
                false,
                volatile,
            )
            .await;
            delivery.nack(false).await?;
            return Ok(());
        }
    };
    let outputs = match current.outputs {
        Some(outputs) => outputs,
        None => {
            tracing::warn!(
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type(),
                resource_name = %message.resource.resource_name(),
                "missing current outputs for check",
            );
            publish_report(
                &ctx.rq_publisher,
                res_qid_typed,
                start.elapsed().as_millis() as u64,
                outcome_failure(
                    rq::IncidentCategory::InconsistentState,
                    "missing current outputs for check",
                ),
                rq::ResourceOperationalState::Live,
                false,
                volatile,
            )
            .await;
            delivery.nack(false).await?;
            return Ok(());
        }
    };

    let Some((plugin_name, mut plugin)) =
        resolve_plugin(message.resource.resource_type(), &ctx.plugins)
    else {
        tracing::warn!(
            environment_qid = %message.resource.environment_qid,
            resource_type = %message.resource.resource_type(),
            "no plugin available for resource type on check",
        );
        publish_report(
            &ctx.rq_publisher,
            res_qid_typed,
            start.elapsed().as_millis() as u64,
            outcome_failure(
                rq::IncidentCategory::SystemError,
                format!(
                    "no plugin available for resource type {}",
                    message.resource.resource_type()
                ),
            ),
            rq::ResourceOperationalState::Live,
            false,
            volatile,
        )
        .await;
        delivery.nack(false).await?;
        return Ok(());
    };

    let id = resource_id_from_ref(&message.resource);
    let res_qid = resource_qid_string(&message.resource);
    let dep_short = message.deployment_id.short();
    let resource = sclc::Resource {
        inputs,
        outputs,
        dependencies: current
            .dependencies
            .iter()
            .map(|d| ids::ResourceId {
                typ: d.typ.clone(),
                name: d.name.clone(),
            })
            .collect(),
        markers: current.markers,
    };

    tracing::info!(
        plugin = %plugin_name,
        environment_qid = %message.resource.environment_qid,
        resource_type = %message.resource.resource_type(),
        resource_name = %message.resource.resource_name(),
        "calling plugin check",
    );
    let checked = match plugin.check(&dep_qid, id.clone(), resource).await {
        Ok(resource) => resource,
        Err(error) => {
            tracing::warn!(
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type(),
                resource_name = %message.resource.resource_name(),
                error = %error,
                "check plugin call failed",
            );
            log_event(
                &ctx.ldb_publisher,
                &dep_qid,
                &res_qid,
                Severity::Error,
                format!("{}: Failed to check {}: {}", dep_short, id, error),
                format!("{}: Failed to check: {}", dep_short, error),
            )
            .await;
            publish_report(
                &ctx.rq_publisher,
                res_qid_typed,
                start.elapsed().as_millis() as u64,
                outcome_failure(rq::IncidentCategory::SystemError, error),
                rq::ResourceOperationalState::Live,
                false,
                volatile,
            )
            .await;
            delivery.nack(false).await?;
            return Ok(());
        }
    };

    volatile = is_volatile_markers(&checked.markers);

    // Validate plugin output size before persistence (3.3).
    if let Ok(output_json) = serde_json::to_value(&checked.outputs)
        && !check_json_size(&output_json, MAX_OUTPUT_SIZE_BYTES)
    {
        tracing::warn!(
            environment_qid = %message.resource.environment_qid,
            resource_type = %message.resource.resource_type(),
            resource_name = %message.resource.resource_name(),
            "plugin check output exceeds size limit",
        );
        publish_report(
            &ctx.rq_publisher,
            res_qid_typed,
            start.elapsed().as_millis() as u64,
            outcome_failure(
                rq::IncidentCategory::SystemError,
                "plugin check output exceeds size limit",
            ),
            rq::ResourceOperationalState::Live,
            false,
            volatile,
        )
        .await;
        delivery.nack(false).await?;
        return Ok(());
    }

    if let Err(error) = resource_client.set_output(checked.outputs).await {
        tracing::warn!(
            environment_qid = %message.resource.environment_qid,
            resource_type = %message.resource.resource_type(),
            resource_name = %message.resource.resource_name(),
            error = %error,
            "failed to persist checked resource outputs",
        );
        log_event(
            &ctx.ldb_publisher,
            &dep_qid,
            &res_qid,
            Severity::Error,
            format!("{}: Failed to check {}: {}", dep_short, id, error),
            format!("{}: Failed to check: {}", dep_short, error),
        )
        .await;
        publish_report(
            &ctx.rq_publisher,
            res_qid_typed,
            start.elapsed().as_millis() as u64,
            outcome_failure(rq::IncidentCategory::SystemError, error),
            rq::ResourceOperationalState::Live,
            false,
            volatile,
        )
        .await;
        delivery.nack(false).await?;
        return Ok(());
    }

    tracing::info!(
        environment_qid = %message.resource.environment_qid,
        resource_type = %message.resource.resource_type(),
        resource_name = %message.resource.resource_name(),
        "checked resource",
    );
    publish_report(
        &ctx.rq_publisher,
        res_qid_typed,
        start.elapsed().as_millis() as u64,
        outcome_success(),
        rq::ResourceOperationalState::Live,
        false,
        volatile,
    )
    .await;
    Ok(())
}

fn message_meta(message: &rtq::Message) -> (&'static str, &rtq::ResourceRef) {
    match message {
        rtq::Message::Create(msg) => ("CREATE", &msg.resource),
        rtq::Message::Destroy(msg) => ("DESTROY", &msg.resource),
        rtq::Message::Adopt(msg) => ("ADOPT", &msg.resource),
        rtq::Message::Restore(msg) => ("RESTORE", &msg.resource),
        rtq::Message::Check(msg) => ("CHECK", &msg.resource),
    }
}

async fn dial_plugins(
    plugin_specs: &[PluginSpec],
) -> anyhow::Result<HashMap<sclc::ModuleId, rtp::PluginClient>> {
    let mut plugins = HashMap::new();
    for spec in plugin_specs {
        let client = rtp::dial(spec.target.clone()).await?;
        tracing::info!(
            name = %spec.name,
            target = ?spec.target,
            "dialed plugin",
        );
        plugins.insert(spec.name.clone(), client);
    }
    Ok(plugins)
}

fn plugin_name_for_resource_type(resource_type: &str) -> Option<&str> {
    resource_type.split_once('.').map(|(prefix, _)| prefix)
}

fn dependencies_from_refs(dependencies: &[rtq::ResourceRef]) -> Vec<ids::ResourceId> {
    dependencies
        .iter()
        .map(|dependency| dependency.resource_id.clone())
        .collect()
}

async fn log_deployment_event(
    publisher: &ldb::Publisher,
    deployment_qid: &str,
    severity: Severity,
    message: String,
) {
    let namespace_publisher = match publisher.namespace(deployment_qid.to_string()).await {
        Ok(namespace_publisher) => namespace_publisher,
        Err(error) => {
            tracing::warn!(
                deployment_qid,
                error = %error,
                "failed to prepare deployment log publisher",
            );
            return;
        }
    };
    if let Err(error) = namespace_publisher.log(severity, message).await {
        tracing::warn!(
            deployment_qid,
            error = %error,
            "failed to publish deployment log event",
        );
    }
}

async fn log_resource_event(
    publisher: &ldb::Publisher,
    resource_qid: &str,
    severity: Severity,
    message: String,
) {
    let namespace_publisher = match publisher.namespace(resource_qid.to_string()).await {
        Ok(namespace_publisher) => namespace_publisher,
        Err(error) => {
            tracing::warn!(
                resource_qid,
                error = %error,
                "failed to prepare resource log publisher",
            );
            return;
        }
    };
    if let Err(error) = namespace_publisher.log(severity, message).await {
        tracing::warn!(
            resource_qid,
            error = %error,
            "failed to publish resource log event",
        );
    }
}

async fn log_event(
    publisher: &ldb::Publisher,
    deployment_qid: &str,
    resource_qid: &str,
    severity: Severity,
    deployment_message: String,
    resource_message: String,
) {
    log_deployment_event(publisher, deployment_qid, severity, deployment_message).await;
    log_resource_event(publisher, resource_qid, severity, resource_message).await;
}

/// Publish a [`rq::Report`] for a processed resource transition.
///
/// Reporting failures are logged at warn-level and otherwise swallowed —
/// reporting is best-effort from the producer's perspective and must never
/// affect the transition outcome itself.
async fn publish_report(
    publisher: &rq::Publisher,
    resource_qid: ids::ResourceQid,
    elapsed_ms: u64,
    outcome: rq::Outcome,
    operational_state: rq::ResourceOperationalState,
    terminal: bool,
    volatile: bool,
) {
    let report = rq::Report {
        entity_qid: rq::EntityQid::Resource(resource_qid.clone()),
        timestamp: chrono::Utc::now(),
        outcome,
        metrics: rq::Metrics::wall_time(elapsed_ms),
        extension: rq::EntityExtension::Resource(rq::ResourceExtension {
            operational_state,
            terminal,
            volatile,
        }),
    };
    if let Err(error) = publisher.enqueue(&report).await {
        tracing::warn!(
            resource_qid = %resource_qid,
            error = %error,
            "failed to publish rq report",
        );
    }
}

/// Returns `true` if the marker set indicates a volatile resource (i.e. one
/// the plugin has marked as externally mutable). Threaded through every
/// [`publish_report`] call so the RE watchdog only expects heartbeats from
/// resources that actually receive periodic Check messages.
fn is_volatile_markers(markers: &std::collections::BTreeSet<sclc::Marker>) -> bool {
    markers.contains(&sclc::Marker::Volatile)
}

/// Build a successful [`rq::Outcome`].
fn outcome_success() -> rq::Outcome {
    rq::Outcome::Success
}

/// Build a failure [`rq::Outcome`] from a category and a displayable error.
fn outcome_failure(category: rq::IncidentCategory, error: impl ToString) -> rq::Outcome {
    rq::Outcome::Failure {
        category,
        error_message: error.to_string(),
    }
}

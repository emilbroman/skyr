use clap::Parser;
use ldb::Severity;
use std::collections::HashMap;
use std::str::FromStr;
use tokio::task;
use tracing::Instrument;

#[derive(Parser)]
enum Program {
    Daemon {
        #[clap(long = "rdb-hostname", default_value = "localhost")]
        rdb_hostname: String,

        #[clap(long = "rtq-hostname", default_value = "localhost")]
        rtq_hostname: String,

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

        let name = name_str
            .parse::<sclc::ModuleId>()
            .map_err(|error| format!("invalid plugin name `{name_str}`: {error}"))?;
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
            let rdb_client = rdb::ClientBuilder::new()
                .known_node(&rdb_hostname)
                .build()
                .await?;
            let ldb_publisher = ldb::ClientBuilder::new()
                .brokers(format!("{}:9092", ldb_hostname))
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
    plugins: HashMap<String, rtp::PluginClient>,
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
        resource_type = %resource.resource_type,
        resource_name = %resource.resource_name,
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
        resource_type = %resource.resource_type,
        resource_name = %resource.resource_name,
        "acknowledged rtq message",
    );
    Ok(true)
}

fn deployment_qid(environment_qid: &str, deployment_id: &str) -> String {
    format!("{environment_qid}@{deployment_id}")
}

fn resource_qid_string(resource: &rtq::ResourceRef) -> String {
    let resource_id = ids::ResourceId::new(&resource.resource_type, &resource.resource_name);
    match resource.environment_qid.parse::<ids::EnvironmentQid>() {
        Ok(env_qid) => ids::ResourceQid::new(env_qid, resource_id).to_string(),
        Err(_) => format!("{}::{}", resource.environment_qid, resource_id),
    }
}

fn resolve_plugin<'a>(
    resource_type: &'a str,
    plugins: &'a HashMap<String, rtp::PluginClient>,
) -> Option<(&'a str, rtp::PluginClient)> {
    let plugin_name = plugin_name_for_resource_type(resource_type)?;
    let client = plugins.get(plugin_name)?;
    Some((plugin_name, client.clone()))
}

fn resource_id_from_ref(resource: &rtq::ResourceRef) -> ids::ResourceId {
    ids::ResourceId {
        typ: resource.resource_type.clone(),
        name: resource.resource_name.clone(),
    }
}

async fn handle_create(
    message: &rtq::CreateMessage,
    delivery: &rtq::Delivery,
    ctx: &WorkerContext,
) -> anyhow::Result<()> {
    let resource_client = ctx
        .rdb_client
        .namespace(message.resource.environment_qid.clone())
        .resource(
            message.resource.resource_type.clone(),
            message.resource.resource_name.clone(),
        );
    match resource_client.get().await {
        Ok(Some(_)) => {
            tracing::info!(
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type,
                resource_name = %message.resource.resource_name,
                "dropping idempotent create for existing resource",
            );
            delivery.ack().await?;
            return Ok(());
        }
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type,
                resource_name = %message.resource.resource_name,
                error = %error,
                "failed to read resource state before create",
            );
            delivery.nack(false).await?;
            return Ok(());
        }
    }

    let Some((plugin_name, mut plugin)) =
        resolve_plugin(&message.resource.resource_type, &ctx.plugins)
    else {
        tracing::warn!(
            environment_qid = %message.resource.environment_qid,
            resource_type = %message.resource.resource_type,
            "no plugin available for resource type on create",
        );
        delivery.nack(false).await?;
        return Ok(());
    };

    let inputs: sclc::Record = match serde_json::from_value(message.inputs.clone()) {
        Ok(inputs) => inputs,
        Err(error) => {
            tracing::warn!(
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type,
                resource_name = %message.resource.resource_name,
                error = %error,
                "invalid create inputs json",
            );
            delivery.nack(false).await?;
            return Ok(());
        }
    };

    let id = resource_id_from_ref(&message.resource);
    let dep_qid = deployment_qid(&message.resource.environment_qid, &message.deployment_id);
    let res_qid = resource_qid_string(&message.resource);
    let dep_short = &message.deployment_id[..8];

    tracing::info!(
        plugin = plugin_name,
        environment_qid = %message.resource.environment_qid,
        resource_type = %message.resource.resource_type,
        resource_name = %message.resource.resource_name,
        "calling plugin create_resource",
    );
    log_event(
        &ctx.ldb_publisher,
        &dep_qid,
        &res_qid,
        Severity::Info,
        format!("Creating {} for {}", id, dep_short),
        format!("Creating for {}", dep_short),
    )
    .await;

    let resource = match plugin
        .create_resource(
            &message.resource.environment_qid,
            &message.deployment_id,
            id,
            inputs,
        )
        .await
    {
        Ok(resource) => resource,
        Err(error) => {
            tracing::warn!(
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type,
                resource_name = %message.resource.resource_name,
                error = %error,
                "create_resource plugin call failed",
            );
            delivery.nack(false).await?;
            return Ok(());
        }
    };

    if let Err(error) = resource_client
        .set_input(resource.inputs.clone(), dep_qid.clone())
        .await
    {
        tracing::warn!(
            environment_qid = %message.resource.environment_qid,
            resource_type = %message.resource.resource_type,
            resource_name = %message.resource.resource_name,
            error = %error,
            "failed to persist created resource inputs",
        );
        delivery.nack(false).await?;
        return Ok(());
    }

    let dependencies = dependencies_from_refs(&message.dependencies);
    if let Err(error) = resource_client.set_dependencies(&dependencies).await {
        tracing::warn!(
            environment_qid = %message.resource.environment_qid,
            resource_type = %message.resource.resource_type,
            resource_name = %message.resource.resource_name,
            error = %error,
            "failed to persist created resource dependencies",
        );
        delivery.nack(false).await?;
        return Ok(());
    }

    if let Err(error) = resource_client.set_output(resource.outputs).await {
        tracing::warn!(
            environment_qid = %message.resource.environment_qid,
            resource_type = %message.resource.resource_type,
            resource_name = %message.resource.resource_name,
            error = %error,
            "failed to persist created resource outputs",
        );
        delivery.nack(false).await?;
        return Ok(());
    }

    if let Err(error) = resource_client.set_markers(&resource.markers).await {
        tracing::warn!(
            environment_qid = %message.resource.environment_qid,
            resource_type = %message.resource.resource_type,
            resource_name = %message.resource.resource_name,
            error = %error,
            "failed to persist created resource markers",
        );
        delivery.nack(false).await?;
        return Ok(());
    }

    if let Err(error) = resource_client
        .set_source_trace(&message.source_trace)
        .await
    {
        tracing::warn!(
            environment_qid = %message.resource.environment_qid,
            resource_type = %message.resource.resource_type,
            resource_name = %message.resource.resource_name,
            error = %error,
            "failed to persist created resource source trace",
        );
        delivery.nack(false).await?;
        return Ok(());
    }

    tracing::info!(
        environment_qid = %message.resource.environment_qid,
        resource_type = %message.resource.resource_type,
        resource_name = %message.resource.resource_name,
        "created resource",
    );
    log_event(
        &ctx.ldb_publisher,
        &dep_qid,
        &res_qid,
        Severity::Info,
        format!(
            "Created {} for {}",
            ids::ResourceId::new(
                &message.resource.resource_type,
                &message.resource.resource_name
            ),
            dep_short,
        ),
        format!("Created for {}", dep_short),
    )
    .await;
    Ok(())
}

async fn handle_destroy(
    message: &rtq::DestroyMessage,
    delivery: &rtq::Delivery,
    ctx: &WorkerContext,
) -> anyhow::Result<()> {
    let resource_client = ctx
        .rdb_client
        .namespace(message.resource.environment_qid.clone())
        .resource(
            message.resource.resource_type.clone(),
            message.resource.resource_name.clone(),
        );
    let dep_qid = deployment_qid(&message.resource.environment_qid, &message.deployment_id);

    let current = match resource_client.get().await {
        Ok(Some(resource)) => {
            if resource.owner.as_deref() != Some(dep_qid.as_str()) {
                tracing::info!(
                    environment_qid = %message.resource.environment_qid,
                    resource_type = %message.resource.resource_type,
                    resource_name = %message.resource.resource_name,
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
            tracing::info!(
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type,
                resource_name = %message.resource.resource_name,
                "dropping idempotent delete for missing resource",
            );
            delivery.ack().await?;
            return Ok(());
        }
        Err(error) => {
            tracing::warn!(
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type,
                resource_name = %message.resource.resource_name,
                error = %error,
                "failed to read resource state before delete",
            );
            delivery.nack(false).await?;
            return Ok(());
        }
    };

    let inputs = match current.inputs {
        Some(inputs) => inputs,
        None => {
            tracing::warn!(
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type,
                resource_name = %message.resource.resource_name,
                "missing current inputs for delete",
            );
            delivery.nack(false).await?;
            return Ok(());
        }
    };
    let outputs = match current.outputs {
        Some(outputs) => outputs,
        None => {
            tracing::warn!(
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type,
                resource_name = %message.resource.resource_name,
                "missing current outputs for delete",
            );
            delivery.nack(false).await?;
            return Ok(());
        }
    };

    let Some((plugin_name, mut plugin)) =
        resolve_plugin(&message.resource.resource_type, &ctx.plugins)
    else {
        tracing::warn!(
            environment_qid = %message.resource.environment_qid,
            resource_type = %message.resource.resource_type,
            "no plugin available for resource type on delete",
        );
        delivery.nack(false).await?;
        return Ok(());
    };

    let id = resource_id_from_ref(&message.resource);
    let res_qid = resource_qid_string(&message.resource);
    let dep_short = &message.deployment_id[..8];

    tracing::info!(
        plugin = plugin_name,
        environment_qid = %message.resource.environment_qid,
        resource_type = %message.resource.resource_type,
        resource_name = %message.resource.resource_name,
        "calling plugin delete_resource",
    );
    log_event(
        &ctx.ldb_publisher,
        &dep_qid,
        &res_qid,
        Severity::Info,
        format!("Destroying {} for {}", id, dep_short),
        format!("Destroying for {}", dep_short),
    )
    .await;

    if let Err(error) = plugin
        .delete_resource(
            &message.resource.environment_qid,
            &message.deployment_id,
            id,
            inputs,
            outputs,
        )
        .await
    {
        tracing::warn!(
            environment_qid = %message.resource.environment_qid,
            resource_type = %message.resource.resource_type,
            resource_name = %message.resource.resource_name,
            error = %error,
            "delete_resource plugin call failed",
        );
        delivery.nack(false).await?;
        return Ok(());
    }

    if let Err(error) = resource_client.delete().await {
        tracing::warn!(
            environment_qid = %message.resource.environment_qid,
            resource_type = %message.resource.resource_type,
            resource_name = %message.resource.resource_name,
            error = %error,
            "failed to delete resource from rdb",
        );
        delivery.nack(false).await?;
        return Ok(());
    }

    tracing::info!(
        environment_qid = %message.resource.environment_qid,
        resource_type = %message.resource.resource_type,
        resource_name = %message.resource.resource_name,
        "deleted resource",
    );
    log_event(
        &ctx.ldb_publisher,
        &dep_qid,
        &res_qid,
        Severity::Info,
        format!(
            "Destroyed {} for {}",
            ids::ResourceId::new(
                &message.resource.resource_type,
                &message.resource.resource_name
            ),
            dep_short
        ),
        format!("Destroyed for {}", dep_short),
    )
    .await;
    Ok(())
}

struct UpdateParams<'a> {
    resource: &'a rtq::ResourceRef,
    owner_deployment_qid: &'a str,
    target_deployment_id: &'a str,
    desired_inputs: serde_json::Value,
    dependencies: &'a [rtq::ResourceRef],
    operation: &'a str,
    source_trace: &'a ids::SourceTrace,
}

async fn handle_update_inputs(
    params: UpdateParams<'_>,
    delivery: &rtq::Delivery,
    ctx: &WorkerContext,
) -> anyhow::Result<Option<String>> {
    let resource = params.resource;
    let owner_deployment_qid = params.owner_deployment_qid;
    let target_deployment_id = params.target_deployment_id;
    let desired_inputs = params.desired_inputs;
    let dependencies = params.dependencies;
    let operation = params.operation;
    let resource_client = ctx
        .rdb_client
        .namespace(resource.environment_qid.clone())
        .resource(
            resource.resource_type.clone(),
            resource.resource_name.clone(),
        );
    let current = match resource_client.get().await {
        Ok(Some(r)) => r,
        Ok(None) => {
            tracing::info!(
                environment_qid = %resource.environment_qid,
                resource_type = %resource.resource_type,
                resource_name = %resource.resource_name,
                "dropping {operation} for missing resource",
            );
            delivery.ack().await?;
            return Ok(None);
        }
        Err(error) => {
            tracing::warn!(
                environment_qid = %resource.environment_qid,
                resource_type = %resource.resource_type,
                resource_name = %resource.resource_name,
                error = %error,
                "failed to read resource state before {operation}",
            );
            delivery.nack(false).await?;
            return Ok(None);
        }
    };

    if current.owner.as_deref() != Some(owner_deployment_qid) {
        tracing::info!(
            environment_qid = %resource.environment_qid,
            resource_type = %resource.resource_type,
            resource_name = %resource.resource_name,
            message_owner = %owner_deployment_qid,
            current_owner = ?current.owner,
            "dropping {operation} for non-matching owner",
        );
        delivery.ack().await?;
        return Ok(None);
    }

    let prev_inputs = match current.inputs {
        Some(inputs) => inputs,
        None => {
            tracing::warn!(
                environment_qid = %resource.environment_qid,
                resource_type = %resource.resource_type,
                resource_name = %resource.resource_name,
                "missing current inputs for {operation}",
            );
            delivery.nack(false).await?;
            return Ok(None);
        }
    };
    let prev_outputs = match current.outputs.clone() {
        Some(outputs) => outputs,
        None => {
            tracing::warn!(
                environment_qid = %resource.environment_qid,
                resource_type = %resource.resource_type,
                resource_name = %resource.resource_name,
                "missing current outputs for {operation}",
            );
            delivery.nack(false).await?;
            return Ok(None);
        }
    };
    let desired_inputs: sclc::Record = match serde_json::from_value(desired_inputs) {
        Ok(inputs) => inputs,
        Err(error) => {
            tracing::warn!(
                environment_qid = %resource.environment_qid,
                resource_type = %resource.resource_type,
                resource_name = %resource.resource_name,
                error = %error,
                "invalid {operation} desired_inputs json",
            );
            delivery.nack(false).await?;
            return Ok(None);
        }
    };

    let mut inputs_to_persist = prev_inputs.clone();
    let mut outputs_to_persist = current.outputs.clone();
    let mut markers_to_persist = None;

    if prev_inputs != desired_inputs {
        let Some((plugin_name, mut plugin)) = resolve_plugin(&resource.resource_type, &ctx.plugins)
        else {
            tracing::warn!(
                environment_qid = %resource.environment_qid,
                resource_type = %resource.resource_type,
                "no plugin available for resource type on {operation}",
            );
            delivery.nack(false).await?;
            return Ok(None);
        };

        let id = resource_id_from_ref(resource);
        let res_qid = resource_qid_string(resource);
        let target_dep_qid_for_log =
            deployment_qid(&resource.environment_qid, target_deployment_id);
        let dep_short = &target_deployment_id[..8];
        let verb = match operation {
            "adopt" => "Adopting",
            "restore" => "Restoring",
            _ => "Updating",
        };
        tracing::info!(
            plugin = plugin_name,
            environment_qid = %resource.environment_qid,
            resource_type = %resource.resource_type,
            resource_name = %resource.resource_name,
            "calling plugin update_resource for {operation}",
        );
        log_event(
            &ctx.ldb_publisher,
            &target_dep_qid_for_log,
            &res_qid,
            Severity::Info,
            format!("{} {} for {}", verb, id, dep_short),
            format!("{} for {}", verb, dep_short),
        )
        .await;

        let updated = match plugin
            .update_resource(
                &resource.environment_qid,
                target_deployment_id,
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
                    resource_type = %resource.resource_type,
                    resource_name = %resource.resource_name,
                    error = %error,
                    "update_resource plugin call failed for {operation}",
                );
                delivery.nack(false).await?;
                return Ok(None);
            }
        };
        inputs_to_persist = updated.inputs;
        outputs_to_persist = Some(updated.outputs);
        markers_to_persist = Some(updated.markers);
    } else {
        tracing::info!(
            environment_qid = %resource.environment_qid,
            resource_type = %resource.resource_type,
            resource_name = %resource.resource_name,
            "skipping plugin update_resource for {operation} with unchanged inputs",
        );
    }

    let target_dep_qid = deployment_qid(&resource.environment_qid, target_deployment_id);

    if let Err(error) = resource_client
        .set_input(inputs_to_persist, target_dep_qid.clone())
        .await
    {
        tracing::warn!(
            environment_qid = %resource.environment_qid,
            resource_type = %resource.resource_type,
            resource_name = %resource.resource_name,
            error = %error,
            "failed to persist {operation} resource inputs",
        );
        delivery.nack(false).await?;
        return Ok(None);
    }
    let deps = dependencies_from_refs(dependencies);
    if let Err(error) = resource_client.set_dependencies(&deps).await {
        tracing::warn!(
            environment_qid = %resource.environment_qid,
            resource_type = %resource.resource_type,
            resource_name = %resource.resource_name,
            error = %error,
            "failed to persist {operation} resource dependencies",
        );
        delivery.nack(false).await?;
        return Ok(None);
    }
    if let Some(outputs) = outputs_to_persist {
        if let Err(error) = resource_client.set_output(outputs).await {
            tracing::warn!(
                environment_qid = %resource.environment_qid,
                resource_type = %resource.resource_type,
                resource_name = %resource.resource_name,
                error = %error,
                "failed to persist {operation} resource outputs",
            );
            delivery.nack(false).await?;
            return Ok(None);
        }
    } else {
        tracing::warn!(
            environment_qid = %resource.environment_qid,
            resource_type = %resource.resource_type,
            resource_name = %resource.resource_name,
            "{operation} resource has no outputs to persist",
        );
    }
    if let Some(markers) = markers_to_persist
        && let Err(error) = resource_client.set_markers(&markers).await
    {
        tracing::warn!(
            environment_qid = %resource.environment_qid,
            resource_type = %resource.resource_type,
            resource_name = %resource.resource_name,
            error = %error,
            "failed to persist {operation} resource markers",
        );
        delivery.nack(false).await?;
        return Ok(None);
    }
    if let Err(error) = resource_client.set_source_trace(params.source_trace).await {
        tracing::warn!(
            environment_qid = %resource.environment_qid,
            resource_type = %resource.resource_type,
            resource_name = %resource.resource_name,
            error = %error,
            "failed to persist {operation} resource source trace",
        );
        delivery.nack(false).await?;
        return Ok(None);
    }

    Ok(Some(target_dep_qid))
}

async fn handle_adopt(
    message: &rtq::AdoptMessage,
    delivery: &rtq::Delivery,
    ctx: &WorkerContext,
) -> anyhow::Result<()> {
    let from_dep_qid = deployment_qid(
        &message.resource.environment_qid,
        &message.from_deployment_id,
    );
    let Some(to_dep_qid) = handle_update_inputs(
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
    )
    .await?
    else {
        return Ok(());
    };

    let res_qid = resource_qid_string(&message.resource);
    let resource_id = ids::ResourceId::new(
        &message.resource.resource_type,
        &message.resource.resource_name,
    );
    let to_short = &message.to_deployment_id[..8];
    let from_short = &message.from_deployment_id[..8];

    tracing::info!(
        environment_qid = %message.resource.environment_qid,
        resource_type = %message.resource.resource_type,
        resource_name = %message.resource.resource_name,
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
    Ok(())
}

async fn handle_restore(
    message: &rtq::RestoreMessage,
    delivery: &rtq::Delivery,
    ctx: &WorkerContext,
) -> anyhow::Result<()> {
    let dep_qid = deployment_qid(&message.resource.environment_qid, &message.deployment_id);
    let Some(dep_qid) = handle_update_inputs(
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
    )
    .await?
    else {
        return Ok(());
    };

    let res_qid = resource_qid_string(&message.resource);
    let dep_short = &message.deployment_id[..8];

    tracing::info!(
        environment_qid = %message.resource.environment_qid,
        resource_type = %message.resource.resource_type,
        resource_name = %message.resource.resource_name,
        owner = %dep_qid,
        "restored resource",
    );
    log_event(
        &ctx.ldb_publisher,
        &dep_qid,
        &res_qid,
        Severity::Info,
        format!(
            "Restored {} for {}",
            ids::ResourceId::new(
                &message.resource.resource_type,
                &message.resource.resource_name
            ),
            dep_short,
        ),
        format!("Restored for {}", dep_short),
    )
    .await;
    Ok(())
}

async fn handle_check(
    message: &rtq::CheckMessage,
    delivery: &rtq::Delivery,
    ctx: &WorkerContext,
) -> anyhow::Result<()> {
    let resource_client = ctx
        .rdb_client
        .namespace(message.resource.environment_qid.clone())
        .resource(
            message.resource.resource_type.clone(),
            message.resource.resource_name.clone(),
        );
    let dep_qid = deployment_qid(&message.resource.environment_qid, &message.deployment_id);

    let current = match resource_client.get().await {
        Ok(Some(resource)) => {
            if resource.owner.as_deref() != Some(dep_qid.as_str()) {
                tracing::info!(
                    environment_qid = %message.resource.environment_qid,
                    resource_type = %message.resource.resource_type,
                    resource_name = %message.resource.resource_name,
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
            tracing::info!(
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type,
                resource_name = %message.resource.resource_name,
                "dropping check for missing resource",
            );
            delivery.ack().await?;
            return Ok(());
        }
        Err(error) => {
            tracing::warn!(
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type,
                resource_name = %message.resource.resource_name,
                error = %error,
                "failed to read resource state before check",
            );
            delivery.nack(false).await?;
            return Ok(());
        }
    };

    let inputs = match current.inputs {
        Some(inputs) => inputs,
        None => {
            tracing::warn!(
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type,
                resource_name = %message.resource.resource_name,
                "missing current inputs for check",
            );
            delivery.nack(false).await?;
            return Ok(());
        }
    };
    let outputs = match current.outputs {
        Some(outputs) => outputs,
        None => {
            tracing::warn!(
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type,
                resource_name = %message.resource.resource_name,
                "missing current outputs for check",
            );
            delivery.nack(false).await?;
            return Ok(());
        }
    };

    let Some((plugin_name, mut plugin)) =
        resolve_plugin(&message.resource.resource_type, &ctx.plugins)
    else {
        tracing::warn!(
            environment_qid = %message.resource.environment_qid,
            resource_type = %message.resource.resource_type,
            "no plugin available for resource type on check",
        );
        delivery.nack(false).await?;
        return Ok(());
    };

    let id = resource_id_from_ref(&message.resource);
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

    let res_qid = resource_qid_string(&message.resource);
    let dep_short = &message.deployment_id[..8];

    tracing::info!(
        plugin = plugin_name,
        environment_qid = %message.resource.environment_qid,
        resource_type = %message.resource.resource_type,
        resource_name = %message.resource.resource_name,
        "calling plugin check",
    );
    log_event(
        &ctx.ldb_publisher,
        &dep_qid,
        &res_qid,
        Severity::Info,
        format!("Checking {} for {}", id, dep_short),
        format!("Checking for {}", dep_short),
    )
    .await;

    let checked = match plugin
        .check(
            &message.resource.environment_qid,
            &message.deployment_id,
            id.clone(),
            resource,
        )
        .await
    {
        Ok(resource) => resource,
        Err(error) => {
            tracing::warn!(
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type,
                resource_name = %message.resource.resource_name,
                error = %error,
                "check plugin call failed",
            );
            delivery.nack(false).await?;
            return Ok(());
        }
    };

    if let Err(error) = resource_client.set_output(checked.outputs).await {
        tracing::warn!(
            environment_qid = %message.resource.environment_qid,
            resource_type = %message.resource.resource_type,
            resource_name = %message.resource.resource_name,
            error = %error,
            "failed to persist checked resource outputs",
        );
        delivery.nack(false).await?;
        return Ok(());
    }

    tracing::info!(
        environment_qid = %message.resource.environment_qid,
        resource_type = %message.resource.resource_type,
        resource_name = %message.resource.resource_name,
        "checked resource",
    );
    log_event(
        &ctx.ldb_publisher,
        &dep_qid,
        &res_qid,
        Severity::Info,
        format!(
            "Checked {} for {}",
            ids::ResourceId::new(
                &message.resource.resource_type,
                &message.resource.resource_name
            ),
            dep_short
        ),
        format!("Checked for {}", dep_short),
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
) -> anyhow::Result<HashMap<String, rtp::PluginClient>> {
    let mut plugins = HashMap::new();
    for spec in plugin_specs {
        let client = rtp::dial(spec.target.clone()).await?;
        tracing::info!(
            name = %spec.name,
            target = ?spec.target,
            "dialed plugin",
        );
        plugins.insert(spec.name.to_string(), client);
    }
    Ok(plugins)
}

fn plugin_name_for_resource_type(resource_type: &str) -> Option<&str> {
    resource_type.split_once('.').map(|(prefix, _)| prefix)
}

fn dependencies_from_refs(dependencies: &[rtq::ResourceRef]) -> Vec<ids::ResourceId> {
    dependencies
        .iter()
        .map(|dependency| ids::ResourceId {
            typ: dependency.resource_type.clone(),
            name: dependency.resource_name.clone(),
        })
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

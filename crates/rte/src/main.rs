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
        let parts = s.split('@').collect::<Vec<_>>();
        if parts.len() != 2 {
            return Err(format!(
                "invalid plugin spec `{s}`: expected NAME@TARGET (e.g. Std/Random@tcp://127.0.0.1:50051)"
            ));
        }

        let name = parts[0]
            .parse::<sclc::ModuleId>()
            .map_err(|error| format!("invalid plugin name `{}`: {error}", parts[0]))?;
        let target = parts[1]
            .parse::<rtp::Target>()
            .map_err(|error| format!("invalid plugin target `{}`: {error}", parts[1]))?;

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

                let span = tracing::info_span!("worker", worker = %format!("{}/{}", index, worker_count));
                tracing::info!(
                    parent: &span,
                    shards = ?consumer.owned_shards(),
                    "started rtq consumer",
                );

                handles.push(task::spawn(
                    worker_loop(
                        consumer,
                        rdb_client.clone(),
                        ldb_publisher.clone(),
                        plugins.clone(),
                    )
                    .instrument(span),
                ));
            }

            for handle in handles {
                handle.await?;
            }

            Ok(())
        }
    }
}

async fn worker_loop(
    mut consumer: rtq::Consumer,
    rdb_client: rdb::Client,
    ldb_publisher: ldb::Publisher,
    plugins: HashMap<String, rtp::PluginClient>,
) {
    loop {
        let keep_running =
            match worker_loop_iteration(&mut consumer, &rdb_client, &ldb_publisher, &plugins)
                .await
            {
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
    rdb_client: &rdb::Client,
    ldb_publisher: &ldb::Publisher,
    plugins: &HashMap<String, rtp::PluginClient>,
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
        resource_id = %resource.resource_id,
        redelivered = delivery.redelivered(),
        "received rtq message",
    );

    match &delivery.message {
        rtq::Message::Create(message) => {
            let resource_client = rdb_client
                .namespace(message.resource.environment_qid.clone())
                .resource(
                    message.resource.resource_type.clone(),
                    message.resource.resource_id.clone(),
                );
            match resource_client.get().await {
                Ok(Some(_)) => {
                    tracing::info!(
                        environment_qid = %message.resource.environment_qid,
                        resource_type = %message.resource.resource_type,
                        resource_id = %message.resource.resource_id,
                        "dropping idempotent create for existing resource",
                    );
                    delivery.ack().await?;
                    return Ok(true);
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(
                        environment_qid = %message.resource.environment_qid,
                        resource_type = %message.resource.resource_type,
                        resource_id = %message.resource.resource_id,
                        error = %error,
                        "failed to read resource state before create",
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                }
            }

            let Some(plugin_name) = plugin_name_for_resource_type(&message.resource.resource_type)
            else {
                tracing::warn!(
                    environment_qid = %message.resource.environment_qid,
                    resource_type = %message.resource.resource_type,
                    "invalid resource type for create",
                );
                delivery.nack(false).await?;
                return Ok(true);
            };
            let Some(mut plugin) = plugins.get(plugin_name).cloned() else {
                tracing::warn!(
                    environment_qid = %message.resource.environment_qid,
                    resource_type = %message.resource.resource_type,
                    plugin = plugin_name,
                    "no plugin configured for resource type",
                );
                delivery.nack(false).await?;
                return Ok(true);
            };

            let inputs: sclc::Record = match serde_json::from_value(message.inputs.clone()) {
                Ok(inputs) => inputs,
                Err(error) => {
                    tracing::warn!(
                        environment_qid = %message.resource.environment_qid,
                        resource_type = %message.resource.resource_type,
                        resource_id = %message.resource.resource_id,
                        error = %error,
                        "invalid create inputs json",
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                }
            };

            let id = sclc::ResourceId {
                ty: message.resource.resource_type.clone(),
                id: message.resource.resource_id.clone(),
            };
            tracing::info!(
                plugin = plugin_name,
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type,
                resource_id = %message.resource.resource_id,
                "calling plugin create_resource",
            );
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
                        resource_id = %message.resource.resource_id,
                        error = %error,
                        "create_resource plugin call failed",
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                }
            };

            let deployment_qid = format!("{}@{}", message.resource.environment_qid, message.deployment_id);

            if let Err(error) = resource_client
                .set_input(resource.inputs.clone(), deployment_qid.clone())
                .await
            {
                tracing::warn!(
                    environment_qid = %message.resource.environment_qid,
                    resource_type = %message.resource.resource_type,
                    resource_id = %message.resource.resource_id,
                    error = %error,
                    "failed to persist created resource inputs",
                );
                delivery.nack(false).await?;
                return Ok(true);
            }

            let dependencies = dependencies_from_refs(&message.dependencies);
            if let Err(error) = resource_client.set_dependencies(&dependencies).await {
                tracing::warn!(
                    environment_qid = %message.resource.environment_qid,
                    resource_type = %message.resource.resource_type,
                    resource_id = %message.resource.resource_id,
                    error = %error,
                    "failed to persist created resource dependencies",
                );
                delivery.nack(false).await?;
                return Ok(true);
            }

            if let Err(error) = resource_client.set_output(resource.outputs).await {
                tracing::warn!(
                    environment_qid = %message.resource.environment_qid,
                    resource_type = %message.resource.resource_type,
                    resource_id = %message.resource.resource_id,
                    error = %error,
                    "failed to persist created resource outputs",
                );
                delivery.nack(false).await?;
                return Ok(true);
            }

            tracing::info!(
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type,
                resource_id = %message.resource.resource_id,
                "created resource",
            );
            log_deployment_event(
                &ldb_publisher,
                &deployment_qid,
                Severity::Info,
                format!(
                    "CREATE applied for {}.{}",
                    message.resource.resource_type, message.resource.resource_id
                ),
            )
            .await;
        }
        rtq::Message::Destroy(message) => {
            let resource_client = rdb_client
                .namespace(message.resource.environment_qid.clone())
                .resource(
                    message.resource.resource_type.clone(),
                    message.resource.resource_id.clone(),
                );
            let deployment_qid = format!("{}@{}", message.resource.environment_qid, message.deployment_id);

            let current = match resource_client.get().await {
                Ok(Some(resource)) => {
                    if resource.owner.as_deref() != Some(deployment_qid.as_str()) {
                        tracing::info!(
                            environment_qid = %message.resource.environment_qid,
                            resource_type = %message.resource.resource_type,
                            resource_id = %message.resource.resource_id,
                            message_owner = %deployment_qid,
                            current_owner = ?resource.owner,
                            "dropping delete for non-matching owner",
                        );
                        delivery.ack().await?;
                        return Ok(true);
                    }
                    resource
                }
                Ok(None) => {
                    tracing::info!(
                        environment_qid = %message.resource.environment_qid,
                        resource_type = %message.resource.resource_type,
                        resource_id = %message.resource.resource_id,
                        "dropping idempotent delete for missing resource",
                    );
                    delivery.ack().await?;
                    return Ok(true);
                }
                Err(error) => {
                    tracing::warn!(
                        environment_qid = %message.resource.environment_qid,
                        resource_type = %message.resource.resource_type,
                        resource_id = %message.resource.resource_id,
                        error = %error,
                        "failed to read resource state before delete",
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                }
            };

            let inputs = match current.inputs {
                Some(inputs) => inputs,
                None => {
                    tracing::warn!(
                        environment_qid = %message.resource.environment_qid,
                        resource_type = %message.resource.resource_type,
                        resource_id = %message.resource.resource_id,
                        "missing current inputs for delete",
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                }
            };
            let outputs = match current.outputs {
                Some(outputs) => outputs,
                None => {
                    tracing::warn!(
                        environment_qid = %message.resource.environment_qid,
                        resource_type = %message.resource.resource_type,
                        resource_id = %message.resource.resource_id,
                        "missing current outputs for delete",
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                }
            };

            let Some(plugin_name) = plugin_name_for_resource_type(&message.resource.resource_type)
            else {
                tracing::warn!(
                    environment_qid = %message.resource.environment_qid,
                    resource_type = %message.resource.resource_type,
                    "invalid resource type for delete",
                );
                delivery.nack(false).await?;
                return Ok(true);
            };
            let Some(mut plugin) = plugins.get(plugin_name).cloned() else {
                tracing::warn!(
                    environment_qid = %message.resource.environment_qid,
                    resource_type = %message.resource.resource_type,
                    plugin = plugin_name,
                    "no plugin configured for resource type",
                );
                delivery.nack(false).await?;
                return Ok(true);
            };

            let id = sclc::ResourceId {
                ty: message.resource.resource_type.clone(),
                id: message.resource.resource_id.clone(),
            };
            tracing::info!(
                plugin = plugin_name,
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type,
                resource_id = %message.resource.resource_id,
                "calling plugin delete_resource",
            );
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
                    resource_id = %message.resource.resource_id,
                    error = %error,
                    "delete_resource plugin call failed",
                );
                delivery.nack(false).await?;
                return Ok(true);
            }

            if let Err(error) = resource_client.delete().await {
                tracing::warn!(
                    environment_qid = %message.resource.environment_qid,
                    resource_type = %message.resource.resource_type,
                    resource_id = %message.resource.resource_id,
                    error = %error,
                    "failed to delete resource from rdb",
                );
                delivery.nack(false).await?;
                return Ok(true);
            }

            tracing::info!(
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type,
                resource_id = %message.resource.resource_id,
                "deleted resource",
            );
            log_deployment_event(
                &ldb_publisher,
                &deployment_qid,
                Severity::Info,
                format!(
                    "DESTROY applied for {}.{}",
                    message.resource.resource_type, message.resource.resource_id
                ),
            )
            .await;
        }
        rtq::Message::Adopt(message) => {
            let resource_client = rdb_client
                .namespace(message.resource.environment_qid.clone())
                .resource(
                    message.resource.resource_type.clone(),
                    message.resource.resource_id.clone(),
                );
            let current = match resource_client.get().await {
                Ok(Some(resource)) => resource,
                Ok(None) => {
                    tracing::info!(
                        environment_qid = %message.resource.environment_qid,
                        resource_type = %message.resource.resource_type,
                        resource_id = %message.resource.resource_id,
                        "dropping adopt for missing resource",
                    );
                    delivery.ack().await?;
                    return Ok(true);
                }
                Err(error) => {
                    tracing::warn!(
                        environment_qid = %message.resource.environment_qid,
                        resource_type = %message.resource.resource_type,
                        resource_id = %message.resource.resource_id,
                        error = %error,
                        "failed to read resource state before adopt",
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                }
            };
            let from_deployment_qid = format!("{}@{}", message.resource.environment_qid, message.from_deployment_id);
            let to_deployment_qid = format!("{}@{}", message.resource.environment_qid, message.to_deployment_id);

            if current.owner.as_deref() != Some(from_deployment_qid.as_str()) {
                tracing::info!(
                    environment_qid = %message.resource.environment_qid,
                    resource_type = %message.resource.resource_type,
                    resource_id = %message.resource.resource_id,
                    from_owner = %from_deployment_qid,
                    current_owner = ?current.owner,
                    "dropping adopt for non-matching owner",
                );
                delivery.ack().await?;
                return Ok(true);
            }

            let prev_inputs = match current.inputs {
                Some(inputs) => inputs,
                None => {
                    tracing::warn!(
                        environment_qid = %message.resource.environment_qid,
                        resource_type = %message.resource.resource_type,
                        resource_id = %message.resource.resource_id,
                        "missing current inputs for adopt",
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                }
            };
            let prev_outputs = match current.outputs.clone() {
                Some(outputs) => outputs,
                None => {
                    tracing::warn!(
                        environment_qid = %message.resource.environment_qid,
                        resource_type = %message.resource.resource_type,
                        resource_id = %message.resource.resource_id,
                        "missing current outputs for adopt",
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                }
            };
            let desired_inputs: sclc::Record =
                match serde_json::from_value(message.desired_inputs.clone()) {
                    Ok(inputs) => inputs,
                    Err(error) => {
                        tracing::warn!(
                            environment_qid = %message.resource.environment_qid,
                            resource_type = %message.resource.resource_type,
                            resource_id = %message.resource.resource_id,
                            error = %error,
                            "invalid adopt desired_inputs json",
                        );
                        delivery.nack(false).await?;
                        return Ok(true);
                    }
                };

            let mut inputs_to_persist = prev_inputs.clone();
            let mut outputs_to_persist = current.outputs.clone();

            if prev_inputs != desired_inputs {
                let Some(plugin_name) =
                    plugin_name_for_resource_type(&message.resource.resource_type)
                else {
                    tracing::warn!(
                        environment_qid = %message.resource.environment_qid,
                        resource_type = %message.resource.resource_type,
                        "invalid resource type for adopt",
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                };
                let Some(mut plugin) = plugins.get(plugin_name).cloned() else {
                    tracing::warn!(
                        environment_qid = %message.resource.environment_qid,
                        resource_type = %message.resource.resource_type,
                        plugin = plugin_name,
                        "no plugin configured for resource type",
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                };

                let id = sclc::ResourceId {
                    ty: message.resource.resource_type.clone(),
                    id: message.resource.resource_id.clone(),
                };
                tracing::info!(
                    plugin = plugin_name,
                    environment_qid = %message.resource.environment_qid,
                    resource_type = %message.resource.resource_type,
                    resource_id = %message.resource.resource_id,
                    "calling plugin update_resource for adopt",
                );
                let resource = match plugin
                    .update_resource(
                        &message.resource.environment_qid,
                        &message.to_deployment_id,
                        id,
                        prev_inputs,
                        prev_outputs,
                        desired_inputs,
                    )
                    .await
                {
                    Ok(resource) => resource,
                    Err(error) => {
                        tracing::warn!(
                            environment_qid = %message.resource.environment_qid,
                            resource_type = %message.resource.resource_type,
                            resource_id = %message.resource.resource_id,
                            error = %error,
                            "update_resource plugin call failed for adopt",
                        );
                        delivery.nack(false).await?;
                        return Ok(true);
                    }
                };
                inputs_to_persist = resource.inputs;
                outputs_to_persist = Some(resource.outputs);
            } else {
                tracing::info!(
                    environment_qid = %message.resource.environment_qid,
                    resource_type = %message.resource.resource_type,
                    resource_id = %message.resource.resource_id,
                    "skipping plugin update_resource for adopt with unchanged inputs",
                );
            }

            if let Err(error) = resource_client
                .set_input(inputs_to_persist, to_deployment_qid.clone())
                .await
            {
                tracing::warn!(
                    environment_qid = %message.resource.environment_qid,
                    resource_type = %message.resource.resource_type,
                    resource_id = %message.resource.resource_id,
                    error = %error,
                    "failed to persist adopted resource inputs",
                );
                delivery.nack(false).await?;
                return Ok(true);
            }
            let dependencies = dependencies_from_refs(&message.dependencies);
            if let Err(error) = resource_client.set_dependencies(&dependencies).await {
                tracing::warn!(
                    environment_qid = %message.resource.environment_qid,
                    resource_type = %message.resource.resource_type,
                    resource_id = %message.resource.resource_id,
                    error = %error,
                    "failed to persist adopted resource dependencies",
                );
                delivery.nack(false).await?;
                return Ok(true);
            }
            if let Some(outputs) = outputs_to_persist {
                if let Err(error) = resource_client.set_output(outputs).await {
                    tracing::warn!(
                        environment_qid = %message.resource.environment_qid,
                        resource_type = %message.resource.resource_type,
                        resource_id = %message.resource.resource_id,
                        error = %error,
                        "failed to persist adopted resource outputs",
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                }
            } else {
                tracing::warn!(
                    environment_qid = %message.resource.environment_qid,
                    resource_type = %message.resource.resource_type,
                    resource_id = %message.resource.resource_id,
                    "adopted resource has no outputs to persist",
                );
            }

            tracing::info!(
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type,
                resource_id = %message.resource.resource_id,
                from_owner = %from_deployment_qid,
                to_owner = %to_deployment_qid,
                "adopted resource",
            );
            let summary = format!(
                "ADOPT applied for {}.{} from {} to {}",
                message.resource.resource_type,
                message.resource.resource_id,
                from_deployment_qid,
                to_deployment_qid
            );
            log_deployment_event(
                &ldb_publisher,
                &from_deployment_qid,
                Severity::Info,
                summary.clone(),
            )
            .await;
            log_deployment_event(
                &ldb_publisher,
                &to_deployment_qid,
                Severity::Info,
                summary,
            )
            .await;
        }
        rtq::Message::Restore(message) => {
            let resource_client = rdb_client
                .namespace(message.resource.environment_qid.clone())
                .resource(
                    message.resource.resource_type.clone(),
                    message.resource.resource_id.clone(),
                );
            let current = match resource_client.get().await {
                Ok(Some(resource)) => resource,
                Ok(None) => {
                    tracing::info!(
                        environment_qid = %message.resource.environment_qid,
                        resource_type = %message.resource.resource_type,
                        resource_id = %message.resource.resource_id,
                        "dropping restore for missing resource",
                    );
                    delivery.ack().await?;
                    return Ok(true);
                }
                Err(error) => {
                    tracing::warn!(
                        environment_qid = %message.resource.environment_qid,
                        resource_type = %message.resource.resource_type,
                        resource_id = %message.resource.resource_id,
                        error = %error,
                        "failed to read resource state before restore",
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                }
            };
            let deployment_qid = format!("{}@{}", message.resource.environment_qid, message.deployment_id);

            if current.owner.as_deref() != Some(deployment_qid.as_str()) {
                tracing::info!(
                    environment_qid = %message.resource.environment_qid,
                    resource_type = %message.resource.resource_type,
                    resource_id = %message.resource.resource_id,
                    message_owner = %deployment_qid,
                    current_owner = ?current.owner,
                    "dropping restore for non-matching owner",
                );
                delivery.ack().await?;
                return Ok(true);
            }

            let prev_inputs = match current.inputs {
                Some(inputs) => inputs,
                None => {
                    tracing::warn!(
                        environment_qid = %message.resource.environment_qid,
                        resource_type = %message.resource.resource_type,
                        resource_id = %message.resource.resource_id,
                        "missing current inputs for restore",
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                }
            };
            let prev_outputs = match current.outputs.clone() {
                Some(outputs) => outputs,
                None => {
                    tracing::warn!(
                        environment_qid = %message.resource.environment_qid,
                        resource_type = %message.resource.resource_type,
                        resource_id = %message.resource.resource_id,
                        "missing current outputs for restore",
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                }
            };
            let desired_inputs: sclc::Record =
                match serde_json::from_value(message.desired_inputs.clone()) {
                    Ok(inputs) => inputs,
                    Err(error) => {
                        tracing::warn!(
                            environment_qid = %message.resource.environment_qid,
                            resource_type = %message.resource.resource_type,
                            resource_id = %message.resource.resource_id,
                            error = %error,
                            "invalid restore desired_inputs json",
                        );
                        delivery.nack(false).await?;
                        return Ok(true);
                    }
                };

            let mut inputs_to_persist = prev_inputs.clone();
            let mut outputs_to_persist = current.outputs.clone();

            if prev_inputs != desired_inputs {
                let Some(plugin_name) =
                    plugin_name_for_resource_type(&message.resource.resource_type)
                else {
                    tracing::warn!(
                        environment_qid = %message.resource.environment_qid,
                        resource_type = %message.resource.resource_type,
                        "invalid resource type for restore",
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                };
                let Some(mut plugin) = plugins.get(plugin_name).cloned() else {
                    tracing::warn!(
                        environment_qid = %message.resource.environment_qid,
                        resource_type = %message.resource.resource_type,
                        plugin = plugin_name,
                        "no plugin configured for resource type",
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                };

                let id = sclc::ResourceId {
                    ty: message.resource.resource_type.clone(),
                    id: message.resource.resource_id.clone(),
                };
                tracing::info!(
                    plugin = plugin_name,
                    environment_qid = %message.resource.environment_qid,
                    resource_type = %message.resource.resource_type,
                    resource_id = %message.resource.resource_id,
                    "calling plugin update_resource for restore",
                );
                let resource = match plugin
                    .update_resource(
                        &message.resource.environment_qid,
                        &message.deployment_id,
                        id,
                        prev_inputs,
                        prev_outputs,
                        desired_inputs,
                    )
                    .await
                {
                    Ok(resource) => resource,
                    Err(error) => {
                        tracing::warn!(
                            environment_qid = %message.resource.environment_qid,
                            resource_type = %message.resource.resource_type,
                            resource_id = %message.resource.resource_id,
                            error = %error,
                            "update_resource plugin call failed for restore",
                        );
                        delivery.nack(false).await?;
                        return Ok(true);
                    }
                };
                inputs_to_persist = resource.inputs;
                outputs_to_persist = Some(resource.outputs);
            } else {
                tracing::info!(
                    environment_qid = %message.resource.environment_qid,
                    resource_type = %message.resource.resource_type,
                    resource_id = %message.resource.resource_id,
                    "skipping plugin update_resource for restore with unchanged inputs",
                );
            }

            if let Err(error) = resource_client
                .set_input(inputs_to_persist, deployment_qid.clone())
                .await
            {
                tracing::warn!(
                    environment_qid = %message.resource.environment_qid,
                    resource_type = %message.resource.resource_type,
                    resource_id = %message.resource.resource_id,
                    error = %error,
                    "failed to persist restored resource inputs",
                );
                delivery.nack(false).await?;
                return Ok(true);
            }
            let dependencies = dependencies_from_refs(&message.dependencies);
            if let Err(error) = resource_client.set_dependencies(&dependencies).await {
                tracing::warn!(
                    environment_qid = %message.resource.environment_qid,
                    resource_type = %message.resource.resource_type,
                    resource_id = %message.resource.resource_id,
                    error = %error,
                    "failed to persist restored resource dependencies",
                );
                delivery.nack(false).await?;
                return Ok(true);
            }
            if let Some(outputs) = outputs_to_persist {
                if let Err(error) = resource_client.set_output(outputs).await {
                    tracing::warn!(
                        environment_qid = %message.resource.environment_qid,
                        resource_type = %message.resource.resource_type,
                        resource_id = %message.resource.resource_id,
                        error = %error,
                        "failed to persist restored resource outputs",
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                }
            } else {
                tracing::warn!(
                    environment_qid = %message.resource.environment_qid,
                    resource_type = %message.resource.resource_type,
                    resource_id = %message.resource.resource_id,
                    "restored resource has no outputs to persist",
                );
            }

            tracing::info!(
                environment_qid = %message.resource.environment_qid,
                resource_type = %message.resource.resource_type,
                resource_id = %message.resource.resource_id,
                owner = %deployment_qid,
                "restored resource",
            );
            log_deployment_event(
                &ldb_publisher,
                &deployment_qid,
                Severity::Info,
                format!(
                    "RESTORE applied for {}.{}",
                    message.resource.resource_type, message.resource.resource_id
                ),
            )
            .await;
        }
    }

    // Placeholder behavior until transition execution is implemented.
    delivery.ack().await?;
    tracing::info!(
        message_type,
        environment_qid = %resource.environment_qid,
        resource_type = %resource.resource_type,
        resource_id = %resource.resource_id,
        "acknowledged rtq message",
    );
    Ok(true)
}

fn message_meta(message: &rtq::Message) -> (&'static str, &rtq::ResourceRef) {
    match message {
        rtq::Message::Create(msg) => ("CREATE", &msg.resource),
        rtq::Message::Destroy(msg) => ("DESTROY", &msg.resource),
        rtq::Message::Adopt(msg) => ("ADOPT", &msg.resource),
        rtq::Message::Restore(msg) => ("RESTORE", &msg.resource),
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

fn dependencies_from_refs(dependencies: &[rtq::ResourceRef]) -> Vec<sclc::ResourceId> {
    dependencies
        .iter()
        .map(|dependency| sclc::ResourceId {
            ty: dependency.resource_type.clone(),
            id: dependency.resource_id.clone(),
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

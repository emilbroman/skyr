use clap::Parser;
use ldb::Severity;
use slog::{Drain, Logger, error, info, o, warn};
use std::collections::HashMap;
use std::str::FromStr;
use tokio::task;

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
    let decorator = slog_term::TermDecorator::new().build();
    let drain = slog_term::FullFormat::new(decorator).build().fuse();
    let drain = slog_async::Async::new(drain).build().fuse();

    let log = slog::Logger::root(drain, o!());

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
            info!(log, "starting resource transition engine daemon");

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
            let plugins = dial_plugins(&log, &plugin).await?;
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

                let worker_log = log.new(o!("worker" => format!("{}/{}", index, worker_count)));
                info!(
                    worker_log,
                    "started rtq consumer";
                    "shards" => format!("{:?}", consumer.owned_shards())
                );

                handles.push(task::spawn(worker_loop(
                    worker_log,
                    consumer,
                    rdb_client.clone(),
                    ldb_publisher.clone(),
                    plugins.clone(),
                )));
            }

            for handle in handles {
                handle.await?;
            }

            Ok(())
        }
    }
}

async fn worker_loop(
    log: Logger,
    mut consumer: rtq::Consumer,
    rdb_client: rdb::Client,
    ldb_publisher: ldb::Publisher,
    plugins: HashMap<String, rtp::PluginClient>,
) {
    loop {
        let keep_running =
            match worker_loop_iteration(&log, &mut consumer, &rdb_client, &ldb_publisher, &plugins)
                .await
            {
                Ok(keep_running) => keep_running,
                Err(error) => {
                    error!(log, "worker loop iteration failed"; "error" => error.to_string());
                    true
                }
            };

        if !keep_running {
            return;
        }
    }
}

async fn worker_loop_iteration(
    log: &Logger,
    consumer: &mut rtq::Consumer,
    rdb_client: &rdb::Client,
    ldb_publisher: &ldb::Publisher,
    plugins: &HashMap<String, rtp::PluginClient>,
) -> anyhow::Result<bool> {
    info!(log, "polling rtq consumer for next message");
    let Some(delivery) = consumer.next().await? else {
        warn!(log, "rtq consumer stream closed");
        return Ok(false);
    };
    let (message_type, resource) = message_meta(&delivery.message);
    info!(log, "received rtq message";
        "message_type" => message_type,
        "namespace" => resource.namespace.clone(),
        "resource_type" => resource.resource_type.clone(),
        "resource_id" => resource.resource_id.clone(),
        "redelivered" => delivery.redelivered()
    );

    match &delivery.message {
        rtq::Message::Create(message) => {
            let resource_client = rdb_client
                .namespace(message.resource.namespace.clone())
                .resource(
                    message.resource.resource_type.clone(),
                    message.resource.resource_id.clone(),
                );
            match resource_client.get().await {
                Ok(Some(_)) => {
                    info!(log, "dropping idempotent create for existing resource";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone(),
                        "resource_id" => message.resource.resource_id.clone()
                    );
                    delivery.ack().await?;
                    return Ok(true);
                }
                Ok(None) => {}
                Err(error) => {
                    warn!(log, "failed to read resource state before create";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone(),
                        "resource_id" => message.resource.resource_id.clone(),
                        "error" => error.to_string()
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                }
            }

            let Some(plugin_name) = plugin_name_for_resource_type(&message.resource.resource_type)
            else {
                warn!(log, "invalid resource type for create";
                    "namespace" => message.resource.namespace.clone(),
                    "resource_type" => message.resource.resource_type.clone()
                );
                delivery.nack(false).await?;
                return Ok(true);
            };
            let Some(mut plugin) = plugins.get(plugin_name).cloned() else {
                warn!(log, "no plugin configured for resource type";
                    "namespace" => message.resource.namespace.clone(),
                    "resource_type" => message.resource.resource_type.clone(),
                    "plugin" => plugin_name.to_owned()
                );
                delivery.nack(false).await?;
                return Ok(true);
            };

            let inputs: sclc::Record = match serde_json::from_value(message.inputs.clone()) {
                Ok(inputs) => inputs,
                Err(error) => {
                    warn!(log, "invalid create inputs json";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone(),
                        "resource_id" => message.resource.resource_id.clone(),
                        "error" => error.to_string()
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                }
            };

            let id = sclc::ResourceId {
                ty: message.resource.resource_type.clone(),
                id: message.resource.resource_id.clone(),
            };
            info!(log, "calling plugin create_resource";
                "plugin" => plugin_name.to_owned(),
                "namespace" => message.resource.namespace.clone(),
                "resource_type" => message.resource.resource_type.clone(),
                "resource_id" => message.resource.resource_id.clone()
            );
            let resource = match plugin.create_resource(id, inputs).await {
                Ok(resource) => resource,
                Err(error) => {
                    warn!(log, "create_resource plugin call failed";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone(),
                        "resource_id" => message.resource.resource_id.clone(),
                        "error" => error.to_string()
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                }
            };

            if let Err(error) = resource_client
                .set_input(resource.inputs.clone(), message.owner_deployment_id.clone())
                .await
            {
                warn!(log, "failed to persist created resource inputs";
                    "namespace" => message.resource.namespace.clone(),
                    "resource_type" => message.resource.resource_type.clone(),
                    "resource_id" => message.resource.resource_id.clone(),
                    "error" => error.to_string()
                );
                delivery.nack(false).await?;
                return Ok(true);
            }

            let dependencies = dependencies_from_refs(&message.dependencies);
            if let Err(error) = resource_client.set_dependencies(&dependencies).await {
                warn!(log, "failed to persist created resource dependencies";
                    "namespace" => message.resource.namespace.clone(),
                    "resource_type" => message.resource.resource_type.clone(),
                    "resource_id" => message.resource.resource_id.clone(),
                    "error" => error.to_string()
                );
                delivery.nack(false).await?;
                return Ok(true);
            }

            if let Err(error) = resource_client.set_output(resource.outputs).await {
                warn!(log, "failed to persist created resource outputs";
                    "namespace" => message.resource.namespace.clone(),
                    "resource_type" => message.resource.resource_type.clone(),
                    "resource_id" => message.resource.resource_id.clone(),
                    "error" => error.to_string()
                );
                delivery.nack(false).await?;
                return Ok(true);
            }

            info!(log, "created resource";
                "namespace" => message.resource.namespace.clone(),
                "resource_type" => message.resource.resource_type.clone(),
                "resource_id" => message.resource.resource_id.clone()
            );
            log_deployment_event(
                &log,
                &ldb_publisher,
                &message.owner_deployment_id,
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
                .namespace(message.resource.namespace.clone())
                .resource(
                    message.resource.resource_type.clone(),
                    message.resource.resource_id.clone(),
                );
            let current = match resource_client.get().await {
                Ok(Some(resource)) => {
                    if resource.owner.as_deref() != Some(message.owner_deployment_id.as_str()) {
                        info!(log, "dropping delete for non-matching owner";
                            "namespace" => message.resource.namespace.clone(),
                            "resource_type" => message.resource.resource_type.clone(),
                            "resource_id" => message.resource.resource_id.clone(),
                            "message_owner" => message.owner_deployment_id.clone(),
                            "current_owner" => resource.owner
                        );
                        delivery.ack().await?;
                        return Ok(true);
                    }
                    resource
                }
                Ok(None) => {
                    info!(log, "dropping idempotent delete for missing resource";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone(),
                        "resource_id" => message.resource.resource_id.clone()
                    );
                    delivery.ack().await?;
                    return Ok(true);
                }
                Err(error) => {
                    warn!(log, "failed to read resource state before delete";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone(),
                        "resource_id" => message.resource.resource_id.clone(),
                        "error" => error.to_string()
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                }
            };

            let inputs = match current.inputs {
                Some(inputs) => inputs,
                None => {
                    warn!(log, "missing current inputs for delete";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone(),
                        "resource_id" => message.resource.resource_id.clone()
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                }
            };
            let outputs = match current.outputs {
                Some(outputs) => outputs,
                None => {
                    warn!(log, "missing current outputs for delete";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone(),
                        "resource_id" => message.resource.resource_id.clone()
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                }
            };

            let Some(plugin_name) = plugin_name_for_resource_type(&message.resource.resource_type)
            else {
                warn!(log, "invalid resource type for delete";
                    "namespace" => message.resource.namespace.clone(),
                    "resource_type" => message.resource.resource_type.clone()
                );
                delivery.nack(false).await?;
                return Ok(true);
            };
            let Some(mut plugin) = plugins.get(plugin_name).cloned() else {
                warn!(log, "no plugin configured for resource type";
                    "namespace" => message.resource.namespace.clone(),
                    "resource_type" => message.resource.resource_type.clone(),
                    "plugin" => plugin_name.to_owned()
                );
                delivery.nack(false).await?;
                return Ok(true);
            };

            let id = sclc::ResourceId {
                ty: message.resource.resource_type.clone(),
                id: message.resource.resource_id.clone(),
            };
            info!(log, "calling plugin delete_resource";
                "plugin" => plugin_name.to_owned(),
                "namespace" => message.resource.namespace.clone(),
                "resource_type" => message.resource.resource_type.clone(),
                "resource_id" => message.resource.resource_id.clone()
            );
            if let Err(error) = plugin.delete_resource(id, inputs, outputs).await {
                warn!(log, "delete_resource plugin call failed";
                    "namespace" => message.resource.namespace.clone(),
                    "resource_type" => message.resource.resource_type.clone(),
                    "resource_id" => message.resource.resource_id.clone(),
                    "error" => error.to_string()
                );
                delivery.nack(false).await?;
                return Ok(true);
            }

            if let Err(error) = resource_client.delete().await {
                warn!(log, "failed to delete resource from rdb";
                    "namespace" => message.resource.namespace.clone(),
                    "resource_type" => message.resource.resource_type.clone(),
                    "resource_id" => message.resource.resource_id.clone(),
                    "error" => error.to_string()
                );
                delivery.nack(false).await?;
                return Ok(true);
            }

            info!(log, "deleted resource";
                "namespace" => message.resource.namespace.clone(),
                "resource_type" => message.resource.resource_type.clone(),
                "resource_id" => message.resource.resource_id.clone()
            );
            log_deployment_event(
                &log,
                &ldb_publisher,
                &message.owner_deployment_id,
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
                .namespace(message.resource.namespace.clone())
                .resource(
                    message.resource.resource_type.clone(),
                    message.resource.resource_id.clone(),
                );
            let current = match resource_client.get().await {
                Ok(Some(resource)) => resource,
                Ok(None) => {
                    info!(log, "dropping adopt for missing resource";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone(),
                        "resource_id" => message.resource.resource_id.clone()
                    );
                    delivery.ack().await?;
                    return Ok(true);
                }
                Err(error) => {
                    warn!(log, "failed to read resource state before adopt";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone(),
                        "resource_id" => message.resource.resource_id.clone(),
                        "error" => error.to_string()
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                }
            };
            if current.owner.as_deref() != Some(message.from_owner_deployment_id.as_str()) {
                info!(log, "dropping adopt for non-matching owner";
                    "namespace" => message.resource.namespace.clone(),
                    "resource_type" => message.resource.resource_type.clone(),
                    "resource_id" => message.resource.resource_id.clone(),
                    "from_owner" => message.from_owner_deployment_id.clone(),
                    "current_owner" => current.owner
                );
                delivery.ack().await?;
                return Ok(true);
            }

            let prev_inputs = match current.inputs {
                Some(inputs) => inputs,
                None => {
                    warn!(log, "missing current inputs for adopt";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone(),
                        "resource_id" => message.resource.resource_id.clone()
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                }
            };
            let prev_outputs = match current.outputs.clone() {
                Some(outputs) => outputs,
                None => {
                    warn!(log, "missing current outputs for adopt";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone(),
                        "resource_id" => message.resource.resource_id.clone()
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                }
            };
            let desired_inputs: sclc::Record =
                match serde_json::from_value(message.desired_inputs.clone()) {
                    Ok(inputs) => inputs,
                    Err(error) => {
                        warn!(log, "invalid adopt desired_inputs json";
                            "namespace" => message.resource.namespace.clone(),
                            "resource_type" => message.resource.resource_type.clone(),
                            "resource_id" => message.resource.resource_id.clone(),
                            "error" => error.to_string()
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
                    warn!(log, "invalid resource type for adopt";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone()
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                };
                let Some(mut plugin) = plugins.get(plugin_name).cloned() else {
                    warn!(log, "no plugin configured for resource type";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone(),
                        "plugin" => plugin_name.to_owned()
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                };

                let id = sclc::ResourceId {
                    ty: message.resource.resource_type.clone(),
                    id: message.resource.resource_id.clone(),
                };
                info!(log, "calling plugin update_resource for adopt";
                    "plugin" => plugin_name.to_owned(),
                    "namespace" => message.resource.namespace.clone(),
                    "resource_type" => message.resource.resource_type.clone(),
                    "resource_id" => message.resource.resource_id.clone()
                );
                let resource = match plugin
                    .update_resource(id, prev_inputs, prev_outputs, desired_inputs)
                    .await
                {
                    Ok(resource) => resource,
                    Err(error) => {
                        warn!(log, "update_resource plugin call failed for adopt";
                            "namespace" => message.resource.namespace.clone(),
                            "resource_type" => message.resource.resource_type.clone(),
                            "resource_id" => message.resource.resource_id.clone(),
                            "error" => error.to_string()
                        );
                        delivery.nack(false).await?;
                        return Ok(true);
                    }
                };
                inputs_to_persist = resource.inputs;
                outputs_to_persist = Some(resource.outputs);
            } else {
                info!(log, "skipping plugin update_resource for adopt with unchanged inputs";
                    "namespace" => message.resource.namespace.clone(),
                    "resource_type" => message.resource.resource_type.clone(),
                    "resource_id" => message.resource.resource_id.clone()
                );
            }

            if let Err(error) = resource_client
                .set_input(inputs_to_persist, message.to_owner_deployment_id.clone())
                .await
            {
                warn!(log, "failed to persist adopted resource inputs";
                    "namespace" => message.resource.namespace.clone(),
                    "resource_type" => message.resource.resource_type.clone(),
                    "resource_id" => message.resource.resource_id.clone(),
                    "error" => error.to_string()
                );
                delivery.nack(false).await?;
                return Ok(true);
            }
            let dependencies = dependencies_from_refs(&message.dependencies);
            if let Err(error) = resource_client.set_dependencies(&dependencies).await {
                warn!(log, "failed to persist adopted resource dependencies";
                    "namespace" => message.resource.namespace.clone(),
                    "resource_type" => message.resource.resource_type.clone(),
                    "resource_id" => message.resource.resource_id.clone(),
                    "error" => error.to_string()
                );
                delivery.nack(false).await?;
                return Ok(true);
            }
            if let Some(outputs) = outputs_to_persist {
                if let Err(error) = resource_client.set_output(outputs).await {
                    warn!(log, "failed to persist adopted resource outputs";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone(),
                        "resource_id" => message.resource.resource_id.clone(),
                        "error" => error.to_string()
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                }
            } else {
                warn!(log, "adopted resource has no outputs to persist";
                    "namespace" => message.resource.namespace.clone(),
                    "resource_type" => message.resource.resource_type.clone(),
                    "resource_id" => message.resource.resource_id.clone()
                );
            }

            info!(log, "adopted resource";
                "namespace" => message.resource.namespace.clone(),
                "resource_type" => message.resource.resource_type.clone(),
                "resource_id" => message.resource.resource_id.clone(),
                "from_owner" => message.from_owner_deployment_id.clone(),
                "to_owner" => message.to_owner_deployment_id.clone()
            );
            let summary = format!(
                "ADOPT applied for {}.{} from {} to {}",
                message.resource.resource_type,
                message.resource.resource_id,
                message.from_owner_deployment_id,
                message.to_owner_deployment_id
            );
            log_deployment_event(
                &log,
                &ldb_publisher,
                &message.from_owner_deployment_id,
                Severity::Info,
                summary.clone(),
            )
            .await;
            log_deployment_event(
                &log,
                &ldb_publisher,
                &message.to_owner_deployment_id,
                Severity::Info,
                summary,
            )
            .await;
        }
        rtq::Message::Restore(message) => {
            let resource_client = rdb_client
                .namespace(message.resource.namespace.clone())
                .resource(
                    message.resource.resource_type.clone(),
                    message.resource.resource_id.clone(),
                );
            let current = match resource_client.get().await {
                Ok(Some(resource)) => resource,
                Ok(None) => {
                    info!(log, "dropping restore for missing resource";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone(),
                        "resource_id" => message.resource.resource_id.clone()
                    );
                    delivery.ack().await?;
                    return Ok(true);
                }
                Err(error) => {
                    warn!(log, "failed to read resource state before restore";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone(),
                        "resource_id" => message.resource.resource_id.clone(),
                        "error" => error.to_string()
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                }
            };
            if current.owner.as_deref() != Some(message.owner_deployment_id.as_str()) {
                info!(log, "dropping restore for non-matching owner";
                    "namespace" => message.resource.namespace.clone(),
                    "resource_type" => message.resource.resource_type.clone(),
                    "resource_id" => message.resource.resource_id.clone(),
                    "message_owner" => message.owner_deployment_id.clone(),
                    "current_owner" => current.owner
                );
                delivery.ack().await?;
                return Ok(true);
            }

            let prev_inputs = match current.inputs {
                Some(inputs) => inputs,
                None => {
                    warn!(log, "missing current inputs for restore";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone(),
                        "resource_id" => message.resource.resource_id.clone()
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                }
            };
            let prev_outputs = match current.outputs.clone() {
                Some(outputs) => outputs,
                None => {
                    warn!(log, "missing current outputs for restore";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone(),
                        "resource_id" => message.resource.resource_id.clone()
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                }
            };
            let desired_inputs: sclc::Record =
                match serde_json::from_value(message.desired_inputs.clone()) {
                    Ok(inputs) => inputs,
                    Err(error) => {
                        warn!(log, "invalid restore desired_inputs json";
                            "namespace" => message.resource.namespace.clone(),
                            "resource_type" => message.resource.resource_type.clone(),
                            "resource_id" => message.resource.resource_id.clone(),
                            "error" => error.to_string()
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
                    warn!(log, "invalid resource type for restore";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone()
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                };
                let Some(mut plugin) = plugins.get(plugin_name).cloned() else {
                    warn!(log, "no plugin configured for resource type";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone(),
                        "plugin" => plugin_name.to_owned()
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                };

                let id = sclc::ResourceId {
                    ty: message.resource.resource_type.clone(),
                    id: message.resource.resource_id.clone(),
                };
                info!(log, "calling plugin update_resource for restore";
                    "plugin" => plugin_name.to_owned(),
                    "namespace" => message.resource.namespace.clone(),
                    "resource_type" => message.resource.resource_type.clone(),
                    "resource_id" => message.resource.resource_id.clone()
                );
                let resource = match plugin
                    .update_resource(id, prev_inputs, prev_outputs, desired_inputs)
                    .await
                {
                    Ok(resource) => resource,
                    Err(error) => {
                        warn!(log, "update_resource plugin call failed for restore";
                            "namespace" => message.resource.namespace.clone(),
                            "resource_type" => message.resource.resource_type.clone(),
                            "resource_id" => message.resource.resource_id.clone(),
                            "error" => error.to_string()
                        );
                        delivery.nack(false).await?;
                        return Ok(true);
                    }
                };
                inputs_to_persist = resource.inputs;
                outputs_to_persist = Some(resource.outputs);
            } else {
                info!(log, "skipping plugin update_resource for restore with unchanged inputs";
                    "namespace" => message.resource.namespace.clone(),
                    "resource_type" => message.resource.resource_type.clone(),
                    "resource_id" => message.resource.resource_id.clone()
                );
            }

            if let Err(error) = resource_client
                .set_input(inputs_to_persist, message.owner_deployment_id.clone())
                .await
            {
                warn!(log, "failed to persist restored resource inputs";
                    "namespace" => message.resource.namespace.clone(),
                    "resource_type" => message.resource.resource_type.clone(),
                    "resource_id" => message.resource.resource_id.clone(),
                    "error" => error.to_string()
                );
                delivery.nack(false).await?;
                return Ok(true);
            }
            let dependencies = dependencies_from_refs(&message.dependencies);
            if let Err(error) = resource_client.set_dependencies(&dependencies).await {
                warn!(log, "failed to persist restored resource dependencies";
                    "namespace" => message.resource.namespace.clone(),
                    "resource_type" => message.resource.resource_type.clone(),
                    "resource_id" => message.resource.resource_id.clone(),
                    "error" => error.to_string()
                );
                delivery.nack(false).await?;
                return Ok(true);
            }
            if let Some(outputs) = outputs_to_persist {
                if let Err(error) = resource_client.set_output(outputs).await {
                    warn!(log, "failed to persist restored resource outputs";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone(),
                        "resource_id" => message.resource.resource_id.clone(),
                        "error" => error.to_string()
                    );
                    delivery.nack(false).await?;
                    return Ok(true);
                }
            } else {
                warn!(log, "restored resource has no outputs to persist";
                    "namespace" => message.resource.namespace.clone(),
                    "resource_type" => message.resource.resource_type.clone(),
                    "resource_id" => message.resource.resource_id.clone()
                );
            }

            info!(log, "restored resource";
                "namespace" => message.resource.namespace.clone(),
                "resource_type" => message.resource.resource_type.clone(),
                "resource_id" => message.resource.resource_id.clone(),
                "owner" => message.owner_deployment_id.clone()
            );
            log_deployment_event(
                &log,
                &ldb_publisher,
                &message.owner_deployment_id,
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
    info!(log, "acknowledged rtq message";
        "message_type" => message_type,
        "namespace" => resource.namespace.clone(),
        "resource_type" => resource.resource_type.clone(),
        "resource_id" => resource.resource_id.clone()
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
    log: &Logger,
    plugin_specs: &[PluginSpec],
) -> anyhow::Result<HashMap<String, rtp::PluginClient>> {
    let mut plugins = HashMap::new();
    for spec in plugin_specs {
        let client = rtp::dial(spec.target.clone()).await?;
        info!(log, "dialed plugin";
            "name" => spec.name.to_string(),
            "target" => format!("{:?}", spec.target)
        );
        plugins.insert(spec.name.to_string(), client);
    }
    Ok(plugins)
}

fn plugin_name_for_resource_type(resource_type: &str) -> Option<&str> {
    resource_type.rsplit_once('.').map(|(prefix, _)| prefix)
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
    log: &Logger,
    publisher: &ldb::Publisher,
    deployment_id: &str,
    severity: Severity,
    message: String,
) {
    let namespace_publisher = match publisher.namespace(deployment_id.to_string()).await {
        Ok(namespace_publisher) => namespace_publisher,
        Err(error) => {
            warn!(log, "failed to prepare deployment log publisher";
                "deployment_id" => deployment_id.to_string(),
                "error" => error.to_string()
            );
            return;
        }
    };
    if let Err(error) = namespace_publisher.log(severity, message).await {
        warn!(log, "failed to publish deployment log event";
            "deployment_id" => deployment_id.to_string(),
            "error" => error.to_string()
        );
    }
}

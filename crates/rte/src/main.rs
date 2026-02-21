use clap::Parser;
use slog::{Drain, Logger, error, info, o, warn};
use std::collections::HashMap;
use std::str::FromStr;
use tokio::task;

#[derive(Parser)]
enum Program {
    Daemon {
        #[clap(long = "db-hostname", default_value = "localhost")]
        db_hostname: String,

        #[clap(long = "mq-hostname", default_value = "localhost")]
        mq_hostname: String,

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
            db_hostname,
            mq_hostname,
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

            let uri = format!("amqp://{}:5672/%2f", mq_hostname);
            let rdb_client = rdb::ClientBuilder::new()
                .known_node(&db_hostname)
                .build()
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
                    plugins.clone(),
                )));
            }

            for handle in handles {
                if let Err(e) = handle.await? {
                    error!(log, "worker loop failed: {e}");
                }
            }

            Ok(())
        }
    }
}

async fn worker_loop(
    log: Logger,
    mut consumer: rtq::Consumer,
    rdb_client: rdb::Client,
    plugins: HashMap<String, rtp::PluginClient>,
) -> anyhow::Result<()> {
    loop {
        let Some(delivery) = consumer.next().await? else {
            warn!(log, "rtq consumer stream closed");
            return Ok(());
        };

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
                        continue;
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
                        continue;
                    }
                }

                let Some(plugin_name) =
                    plugin_name_for_resource_type(&message.resource.resource_type)
                else {
                    warn!(log, "invalid resource type for create";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone()
                    );
                    delivery.nack(false).await?;
                    continue;
                };
                let Some(mut plugin) = plugins.get(plugin_name).cloned() else {
                    warn!(log, "no plugin configured for resource type";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone(),
                        "plugin" => plugin_name.to_owned()
                    );
                    delivery.nack(false).await?;
                    continue;
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
                        continue;
                    }
                };

                let id = sclc::ResourceId {
                    ty: message.resource.resource_type.clone(),
                    id: message.resource.resource_id.clone(),
                };
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
                        continue;
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
                    continue;
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
                    continue;
                }

                if let Err(error) = resource_client.set_output(resource.outputs).await {
                    warn!(log, "failed to persist created resource outputs";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone(),
                        "resource_id" => message.resource.resource_id.clone(),
                        "error" => error.to_string()
                    );
                    delivery.nack(false).await?;
                    continue;
                }

                info!(log, "created resource";
                    "namespace" => message.resource.namespace.clone(),
                    "resource_type" => message.resource.resource_type.clone(),
                    "resource_id" => message.resource.resource_id.clone()
                );
            }
            rtq::Message::Destroy(message) => {
                let resource_client = rdb_client
                    .namespace(message.resource.namespace.clone())
                    .resource(
                        message.resource.resource_type.clone(),
                        message.resource.resource_id.clone(),
                    );
                match resource_client.get().await {
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
                            continue;
                        }
                    }
                    Ok(None) => {
                        info!(log, "dropping idempotent delete for missing resource";
                            "namespace" => message.resource.namespace.clone(),
                            "resource_type" => message.resource.resource_type.clone(),
                            "resource_id" => message.resource.resource_id.clone()
                        );
                        delivery.ack().await?;
                        continue;
                    }
                    Err(error) => {
                        warn!(log, "failed to read resource state before delete";
                            "namespace" => message.resource.namespace.clone(),
                            "resource_type" => message.resource.resource_type.clone(),
                            "resource_id" => message.resource.resource_id.clone(),
                            "error" => error.to_string()
                        );
                        delivery.nack(false).await?;
                        continue;
                    }
                }

                let Some(plugin_name) =
                    plugin_name_for_resource_type(&message.resource.resource_type)
                else {
                    warn!(log, "invalid resource type for delete";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone()
                    );
                    delivery.nack(false).await?;
                    continue;
                };
                let Some(mut plugin) = plugins.get(plugin_name).cloned() else {
                    warn!(log, "no plugin configured for resource type";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone(),
                        "plugin" => plugin_name.to_owned()
                    );
                    delivery.nack(false).await?;
                    continue;
                };

                let id = sclc::ResourceId {
                    ty: message.resource.resource_type.clone(),
                    id: message.resource.resource_id.clone(),
                };
                if let Err(error) = plugin.delete_resource(id).await {
                    warn!(log, "delete_resource plugin call failed";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone(),
                        "resource_id" => message.resource.resource_id.clone(),
                        "error" => error.to_string()
                    );
                    delivery.nack(false).await?;
                    continue;
                }

                if let Err(error) = resource_client.delete().await {
                    warn!(log, "failed to delete resource from rdb";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone(),
                        "resource_id" => message.resource.resource_id.clone(),
                        "error" => error.to_string()
                    );
                    delivery.nack(false).await?;
                    continue;
                }

                info!(log, "deleted resource";
                    "namespace" => message.resource.namespace.clone(),
                    "resource_type" => message.resource.resource_type.clone(),
                    "resource_id" => message.resource.resource_id.clone()
                );
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
                        continue;
                    }
                    Err(error) => {
                        warn!(log, "failed to read resource state before adopt";
                            "namespace" => message.resource.namespace.clone(),
                            "resource_type" => message.resource.resource_type.clone(),
                            "resource_id" => message.resource.resource_id.clone(),
                            "error" => error.to_string()
                        );
                        delivery.nack(false).await?;
                        continue;
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
                    continue;
                }

                let Some(plugin_name) =
                    plugin_name_for_resource_type(&message.resource.resource_type)
                else {
                    warn!(log, "invalid resource type for adopt";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone()
                    );
                    delivery.nack(false).await?;
                    continue;
                };
                let Some(mut plugin) = plugins.get(plugin_name).cloned() else {
                    warn!(log, "no plugin configured for resource type";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone(),
                        "plugin" => plugin_name.to_owned()
                    );
                    delivery.nack(false).await?;
                    continue;
                };

                let prev_inputs = match current.inputs {
                    Some(inputs) => inputs,
                    None => {
                        warn!(log, "missing current inputs for adopt";
                            "namespace" => message.resource.namespace.clone(),
                            "resource_type" => message.resource.resource_type.clone(),
                            "resource_id" => message.resource.resource_id.clone()
                        );
                        delivery.nack(false).await?;
                        continue;
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
                            continue;
                        }
                    };

                let id = sclc::ResourceId {
                    ty: message.resource.resource_type.clone(),
                    id: message.resource.resource_id.clone(),
                };
                let resource = match plugin
                    .update_resource(id, prev_inputs, desired_inputs)
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
                        continue;
                    }
                };

                if let Err(error) = resource_client
                    .set_input(
                        resource.inputs.clone(),
                        message.to_owner_deployment_id.clone(),
                    )
                    .await
                {
                    warn!(log, "failed to persist adopted resource inputs";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone(),
                        "resource_id" => message.resource.resource_id.clone(),
                        "error" => error.to_string()
                    );
                    delivery.nack(false).await?;
                    continue;
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
                    continue;
                }
                if let Err(error) = resource_client.set_output(resource.outputs).await {
                    warn!(log, "failed to persist adopted resource outputs";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone(),
                        "resource_id" => message.resource.resource_id.clone(),
                        "error" => error.to_string()
                    );
                    delivery.nack(false).await?;
                    continue;
                }

                info!(log, "adopted resource";
                    "namespace" => message.resource.namespace.clone(),
                    "resource_type" => message.resource.resource_type.clone(),
                    "resource_id" => message.resource.resource_id.clone(),
                    "from_owner" => message.from_owner_deployment_id.clone(),
                    "to_owner" => message.to_owner_deployment_id.clone()
                );
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
                        continue;
                    }
                    Err(error) => {
                        warn!(log, "failed to read resource state before restore";
                            "namespace" => message.resource.namespace.clone(),
                            "resource_type" => message.resource.resource_type.clone(),
                            "resource_id" => message.resource.resource_id.clone(),
                            "error" => error.to_string()
                        );
                        delivery.nack(false).await?;
                        continue;
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
                    continue;
                }

                let Some(plugin_name) =
                    plugin_name_for_resource_type(&message.resource.resource_type)
                else {
                    warn!(log, "invalid resource type for restore";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone()
                    );
                    delivery.nack(false).await?;
                    continue;
                };
                let Some(mut plugin) = plugins.get(plugin_name).cloned() else {
                    warn!(log, "no plugin configured for resource type";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone(),
                        "plugin" => plugin_name.to_owned()
                    );
                    delivery.nack(false).await?;
                    continue;
                };

                let prev_inputs = match current.inputs {
                    Some(inputs) => inputs,
                    None => {
                        warn!(log, "missing current inputs for restore";
                            "namespace" => message.resource.namespace.clone(),
                            "resource_type" => message.resource.resource_type.clone(),
                            "resource_id" => message.resource.resource_id.clone()
                        );
                        delivery.nack(false).await?;
                        continue;
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
                            continue;
                        }
                    };

                let id = sclc::ResourceId {
                    ty: message.resource.resource_type.clone(),
                    id: message.resource.resource_id.clone(),
                };
                let resource = match plugin
                    .update_resource(id, prev_inputs, desired_inputs)
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
                        continue;
                    }
                };

                if let Err(error) = resource_client
                    .set_input(resource.inputs.clone(), message.owner_deployment_id.clone())
                    .await
                {
                    warn!(log, "failed to persist restored resource inputs";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone(),
                        "resource_id" => message.resource.resource_id.clone(),
                        "error" => error.to_string()
                    );
                    delivery.nack(false).await?;
                    continue;
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
                    continue;
                }
                if let Err(error) = resource_client.set_output(resource.outputs).await {
                    warn!(log, "failed to persist restored resource outputs";
                        "namespace" => message.resource.namespace.clone(),
                        "resource_type" => message.resource.resource_type.clone(),
                        "resource_id" => message.resource.resource_id.clone(),
                        "error" => error.to_string()
                    );
                    delivery.nack(false).await?;
                    continue;
                }

                info!(log, "restored resource";
                    "namespace" => message.resource.namespace.clone(),
                    "resource_type" => message.resource.resource_type.clone(),
                    "resource_id" => message.resource.resource_id.clone(),
                    "owner" => message.owner_deployment_id.clone()
                );
            }
        }

        // Placeholder behavior until transition execution is implemented.
        delivery.ack().await?;
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

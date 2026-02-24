use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    time::Duration,
};

use cdb::{DeploymentClient, DeploymentState};
use clap::Parser;
use futures_util::{StreamExt, TryStreamExt};
use sclc::SourceRepo;
use slog::{Drain, Logger, debug, error, info, o, warn};
use tokio::{
    sync::mpsc,
    sync::oneshot::{self, error::TryRecvError},
    task,
    time::{Instant, sleep_until},
};

#[derive(Parser)]
enum Program {
    Daemon {
        #[clap(long = "cdb-hostname", default_value = "localhost")]
        cdb_hostname: String,

        #[clap(long = "rdb-hostname", default_value = "localhost")]
        rdb_hostname: String,

        #[clap(long = "rtq-hostname", default_value = "localhost")]
        rtq_hostname: String,

        #[clap(long = "ldb-hostname", default_value = "localhost")]
        ldb_hostname: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let decorator = slog_term::TermDecorator::new().build();
    let drain = slog_term::FullFormat::new(decorator).build().fuse();
    let drain = slog_async::Async::new(drain).build().fuse();

    let log = slog::Logger::root(drain, o!());

    match Program::parse() {
        Program::Daemon {
            cdb_hostname,
            rdb_hostname,
            rtq_hostname,
            ldb_hostname,
        } => {
            info!(log, "starting deployment engine daemon");

            let cdb_client = cdb::ClientBuilder::new()
                .known_node(&cdb_hostname)
                .build()
                .await?;

            let rdb_client = rdb::ClientBuilder::new()
                .known_node(&rdb_hostname)
                .build()
                .await?;

            let rtq_publisher = rtq::ClientBuilder::new()
                .uri(format!("amqp://{}:5672/%2f", rtq_hostname))
                .build_publisher()
                .await?;
            let ldb_publisher = ldb::ClientBuilder::new()
                .brokers(format!("{}:9092", ldb_hostname))
                .build_publisher()
                .await?;

            let mut workers = BTreeMap::new();

            loop {
                let next_loop = Instant::now() + Duration::from_secs(20);

                if let Err(e) = process(
                    log.clone(),
                    cdb_client.clone(),
                    rdb_client.clone(),
                    rtq_publisher.clone(),
                    ldb_publisher.clone(),
                    &mut workers,
                )
                .await
                {
                    error!(log, "{e}")
                }

                debug!(
                    log,
                    "will poll for new deployments again in {:.2}s",
                    (next_loop - Instant::now()).as_secs_f64()
                );
                sleep_until(next_loop).await;
            }
        }
    }
}

async fn process(
    log: Logger,
    client: cdb::Client,
    rdb_client: rdb::Client,
    rtq_publisher: rtq::Publisher,
    ldb_publisher: ldb::Publisher,
    workers: &mut BTreeMap<String, oneshot::Sender<()>>,
) -> anyhow::Result<()> {
    let deployments = client.active_deployments().await?.collect::<Vec<_>>().await;

    debug!(log, "found {} deployments", deployments.len());

    let mut untouched = workers.keys().cloned().collect::<BTreeSet<_>>();
    for deployment in deployments {
        let deployment = deployment?;
        let id = deployment.fqid();
        if !untouched.remove(&id) {
            debug!(log, "new deployment to process: {}", deployment.fqid());

            let (tx, rx) = oneshot::channel();
            let namespace = format!("{}/{}", deployment.repository, deployment.id.ref_name);

            let log_publisher = match ldb_publisher.namespace(id.clone()).await {
                Ok(log_publisher) => log_publisher,
                Err(error) => {
                    error!(
                        log,
                        "failed to create deployment log publisher topic";
                        "dep" => id.clone(),
                        "error" => error.to_string()
                    );
                    continue;
                }
            };

            let worker = Worker {
                log: log.new(o!("dep" => deployment.fqid())),
                client: client.repo(deployment.repository).deployment(deployment.id),
                namespace: rdb_client.namespace(namespace),
                rtq_publisher: rtq_publisher.clone(),
                log_publisher,
            };

            task::spawn(worker.run_loop(rx));

            workers.insert(id, tx);
        }
    }

    for id in untouched {
        debug!(log, "no longer watching {}", id);
        workers.remove(&id);
    }

    Ok(())
}

struct Worker {
    log: Logger,
    client: DeploymentClient,
    namespace: rdb::NamespaceClient,
    rtq_publisher: rtq::Publisher,
    log_publisher: ldb::NamespacePublisher,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EvalCompleteness {
    Complete,
    Partial,
}

impl Worker {
    async fn run_loop(mut self, mut rx: oneshot::Receiver<()>) {
        loop {
            let next_loop = Instant::now() + Duration::from_secs(5);

            match rx.try_recv() {
                Ok(()) | Err(TryRecvError::Closed) => return,
                Err(TryRecvError::Empty) => {
                    if let Err(e) = self.work().await {
                        error!(self.log, "{e}");
                    }
                }
            }

            debug!(
                self.log,
                "will reconcile in {:.2}s",
                (next_loop - Instant::now()).as_secs_f64()
            );
            sleep_until(next_loop).await;
        }
    }

    async fn work(&mut self) -> anyhow::Result<()> {
        let deployment = self.client.get().await?;

        match deployment.state {
            DeploymentState::Down => {
                info!(
                    &self.log,
                    "deployment down, waiting to be decommissioned..."
                );
                Ok(())
            }

            DeploymentState::Desired => {
                info!(&self.log, "reconciling");
                let completeness = self.compile_and_evaluate().await?;

                match completeness {
                    EvalCompleteness::Complete => {
                        for superceded in self.client.superceded().await? {
                            let superceded_deployment = superceded.get().await?;
                            if superceded_deployment.state == DeploymentState::Lingering {
                                superceded.set(DeploymentState::Undesired).await?;
                            }
                        }
                    }
                    EvalCompleteness::Partial => {
                        info!(
                            &self.log,
                            "evaluation incomplete; deferring superceded deployment teardown"
                        );
                    }
                }

                Ok(())
            }

            DeploymentState::Undesired => {
                info!(&self.log, "tearing down");

                let owner = deployment.fqid();
                let mut all_resources = Vec::new();
                let mut resources = self.namespace.list_resources().await?;
                while let Some(resource) = resources.try_next().await? {
                    all_resources.push(resource);
                }

                let owned_resources = all_resources
                    .iter()
                    .filter(|resource| resource.owner.as_deref() == Some(owner.as_str()))
                    .collect::<Vec<_>>();
                let mut emitted = 0usize;
                let mut blocked = 0usize;
                let living_dependency_targets = all_resources
                    .iter()
                    .flat_map(|resource| resource.dependencies.iter().cloned())
                    .collect::<HashSet<_>>();

                for resource in &owned_resources {
                    let resource_id = sclc::ResourceId {
                        ty: resource.resource_type.clone(),
                        id: resource.id.clone(),
                    };
                    if living_dependency_targets.contains(&resource_id) {
                        blocked += 1;
                        info!(&self.log, "resource still has living dependents; deferring destroy";
                            "type" => resource.resource_type.clone(),
                            "id" => resource.id.clone(),
                            "owner" => resource.owner.clone()
                        );
                        continue;
                    }

                    let message = rtq::Message::Destroy(rtq::DestroyMessage {
                        resource: rtq::ResourceRef {
                            namespace: self.namespace.namespace().to_owned(),
                            resource_type: resource.resource_type.clone(),
                            resource_id: resource.id.clone(),
                        },
                        owner_deployment_id: owner.clone(),
                    });
                    self.rtq_publisher.enqueue(&message).await?;
                    emitted += 1;

                    info!(&self.log, "queued destroy";
                        "type" => resource.resource_type.clone(),
                        "id" => resource.id.clone(),
                        "owner" => resource.owner.clone()
                    );
                }

                if emitted > 0 {
                    info!(&self.log, "queued {} destroy messages", emitted);
                    return Ok(());
                }

                if owned_resources.is_empty() {
                    info!(&self.log, "no more resources, setting state to DOWN");
                    self.client.set(DeploymentState::Down).await?;
                    self.log_publisher
                        .info(format!("No more resources, deployment is DOWN"))
                        .await;
                    return Ok(());
                }

                if blocked > 0 {
                    info!(&self.log, "teardown waiting on living dependents";
                        "blocked_resources" => blocked
                    );
                    self.log_publisher
                        .info(format!("{blocked} resources still have living dependents"))
                        .await;
                }
                Ok(())
            }

            DeploymentState::Lingering => {
                info!(&self.log, "lingering...");
                let mut cursor = self.client.clone();
                let mut seen = HashSet::new();

                while let Some(superceding) = cursor.get_superceding().await? {
                    let superceding_deployment = superceding.get().await?;
                    let commit_hash = superceding_deployment.id.commit_hash.clone();

                    if !seen.insert(commit_hash) {
                        warn!(&self.log, "detected supercession cycle while lingering");
                        break;
                    }

                    if superceding_deployment.state == DeploymentState::Desired {
                        self.client
                            .mark_superceded_by(superceding_deployment.id.commit_hash)
                            .await?;
                        break;
                    }

                    cursor = superceding;
                }

                Ok(())
            }
        }
    }

    async fn compile_and_evaluate(&mut self) -> anyhow::Result<EvalCompleteness> {
        let diagnosed = sclc::compile(self.client.clone()).await?;
        for diag in diagnosed.diags().iter() {
            let (module_id, span) = diag.locate();
            info!(&self.log, "compile diagnostic";
                "module" => module_id.to_string(),
                "span" => span.to_string(),
                "diag" => diag.to_string()
            );
        }

        for diag in diagnosed.diags().iter() {
            let (module_id, span) = diag.locate();

            self.log_publisher
                .log(
                    match diag.level() {
                        sclc::DiagLevel::Error => ldb::Severity::Error,
                        sclc::DiagLevel::Warning => ldb::Severity::Warning,
                    },
                    format!("{module_id}:{span}: {diag}"),
                )
                .await
                .unwrap_or_default();
        }

        if diagnosed.diags().has_errors() {
            info!(&self.log, "compile produced errors; skipping evaluation");
            return Ok(EvalCompleteness::Partial);
        }

        let mut program = diagnosed.into_inner();
        let module_id = SourceRepo::package_id(&self.client)
            .as_slice()
            .iter()
            .cloned()
            .chain(std::iter::once(String::from("Main")))
            .collect::<sclc::ModuleId>();
        let owner_deployment_id = self.client.fqid();

        let (effects_tx, mut effects_rx) = mpsc::unbounded_channel();
        let mut eval = sclc::Eval::new::<DeploymentClient>(effects_tx, owner_deployment_id.clone());
        let mut unowned_resource_owner_by_id = HashMap::new();
        let mut resources = self.namespace.list_resources().await?;
        while let Some(resource) = resources.try_next().await? {
            let resource_id = sclc::ResourceId {
                ty: resource.resource_type.clone(),
                id: resource.id.clone(),
            };
            if resource.owner.as_deref() != Some(owner_deployment_id.as_str()) {
                if let Some(owner) = resource.owner.clone() {
                    unowned_resource_owner_by_id.insert(resource_id.clone(), owner);
                }
            }

            eval.add_resource(
                resource_id,
                sclc::Resource {
                    inputs: resource.inputs.unwrap_or_default(),
                    outputs: resource.outputs.unwrap_or_default(),
                    dependencies: resource.dependencies,
                },
            );
        }
        drop(resources);

        let log = self.log.clone();
        let log_publisher = self.log_publisher.clone();
        let namespace_id = self.namespace.namespace().to_owned();
        let rtq_publisher = self.rtq_publisher.clone();
        let effects_task = task::spawn(async move {
            let mut had_effect = false;
            while let Some(effect) = effects_rx.recv().await {
                match effect {
                    sclc::Effect::CreateResource {
                        id,
                        inputs,
                        dependencies,
                    } => {
                        had_effect = true;
                        let message = rtq::Message::Create(rtq::CreateMessage {
                            resource: rtq::ResourceRef {
                                namespace: namespace_id.clone(),
                                resource_type: id.ty.clone(),
                                resource_id: id.id.clone(),
                            },
                            owner_deployment_id: owner_deployment_id.clone(),
                            inputs: match serde_json::to_value(&inputs) {
                                Ok(value) => value,
                                Err(error) => {
                                    error!(log, "failed to encode create inputs";
                                        "type" => id.ty,
                                        "id" => id.id,
                                        "error" => error.to_string()
                                    );
                                    continue;
                                }
                            },
                            dependencies: dependencies
                                .into_iter()
                                .map(|dependency| rtq::ResourceRef {
                                    namespace: namespace_id.clone(),
                                    resource_type: dependency.ty,
                                    resource_id: dependency.id,
                                })
                                .collect(),
                        });
                        if let Err(error) = rtq_publisher.enqueue(&message).await {
                            error!(log, "failed to publish create message";
                                "error" => error.to_string()
                            );

                            log_publisher
                                .error(format!(
                                    "Failed to enqueue CREATE {}.{}: {}",
                                    id.ty, id.id, error
                                ))
                                .await;

                            continue;
                        }

                        info!(log, "effect create resource";
                            "type" => id.ty.clone(),
                            "id" => id.id.clone(),
                            "inputs" => format!("{:?}", inputs)
                        );
                    }
                    sclc::Effect::UpdateResource {
                        id,
                        inputs,
                        dependencies,
                    } => {
                        had_effect = true;
                        let desired_inputs = match serde_json::to_value(&inputs) {
                            Ok(value) => value,
                            Err(error) => {
                                error!(log, "failed to encode update inputs";
                                    "type" => id.ty,
                                    "id" => id.id,
                                    "error" => error.to_string()
                                );
                                continue;
                            }
                        };
                        let dependencies = dependencies
                            .into_iter()
                            .map(|dependency| rtq::ResourceRef {
                                namespace: namespace_id.clone(),
                                resource_type: dependency.ty,
                                resource_id: dependency.id,
                            })
                            .collect::<Vec<_>>();
                        let message = if let Some(from_owner_deployment_id) =
                            unowned_resource_owner_by_id.get(&id).cloned()
                        {
                            rtq::Message::Adopt(rtq::AdoptMessage {
                                resource: rtq::ResourceRef {
                                    namespace: namespace_id.clone(),
                                    resource_type: id.ty.clone(),
                                    resource_id: id.id.clone(),
                                },
                                from_owner_deployment_id,
                                to_owner_deployment_id: owner_deployment_id.clone(),
                                desired_inputs,
                                dependencies,
                            })
                        } else {
                            rtq::Message::Restore(rtq::RestoreMessage {
                                resource: rtq::ResourceRef {
                                    namespace: namespace_id.clone(),
                                    resource_type: id.ty.clone(),
                                    resource_id: id.id.clone(),
                                },
                                owner_deployment_id: owner_deployment_id.clone(),
                                desired_inputs,
                                dependencies,
                            })
                        };
                        if let Err(error) = rtq_publisher.enqueue(&message).await {
                            error!(log, "failed to publish update message";
                                "error" => error.to_string()
                            );

                            log_publisher
                                .error(format!(
                                    "Failed to enqueue UPDATE {}.{}: {}",
                                    id.ty, id.id, error
                                ))
                                .await;
                            continue;
                        }

                        info!(log, "effect update resource";
                            "type" => id.ty.clone(),
                            "id" => id.id.clone(),
                            "inputs" => format!("{:?}", inputs)
                        );
                    }
                    sclc::Effect::TouchResource {
                        id,
                        inputs,
                        dependencies,
                    } => {
                        let Some(from_owner_deployment_id) =
                            unowned_resource_owner_by_id.get(&id).cloned()
                        else {
                            continue;
                        };
                        had_effect = true;
                        let desired_inputs = match serde_json::to_value(&inputs) {
                            Ok(value) => value,
                            Err(error) => {
                                error!(log, "failed to encode touch inputs";
                                    "type" => id.ty,
                                    "id" => id.id,
                                    "error" => error.to_string()
                                );
                                continue;
                            }
                        };
                        let dependencies = dependencies
                            .into_iter()
                            .map(|dependency| rtq::ResourceRef {
                                namespace: namespace_id.clone(),
                                resource_type: dependency.ty,
                                resource_id: dependency.id,
                            })
                            .collect::<Vec<_>>();
                        let message = rtq::Message::Adopt(rtq::AdoptMessage {
                            resource: rtq::ResourceRef {
                                namespace: namespace_id.clone(),
                                resource_type: id.ty.clone(),
                                resource_id: id.id.clone(),
                            },
                            from_owner_deployment_id,
                            to_owner_deployment_id: owner_deployment_id.clone(),
                            desired_inputs,
                            dependencies,
                        });
                        if let Err(error) = rtq_publisher.enqueue(&message).await {
                            error!(log, "failed to publish touch adopt message";
                                "error" => error.to_string()
                            );

                            log_publisher
                                .error(format!(
                                    "Failed to enqueue ADOPT {}.{}: {}",
                                    id.ty, id.id, error
                                ))
                                .await;
                            continue;
                        }

                        info!(log, "effect touch resource adopt";
                            "type" => id.ty.clone(),
                            "id" => id.id.clone(),
                            "inputs" => format!("{:?}", inputs)
                        );
                    }
                }
            }

            if had_effect {
                EvalCompleteness::Partial
            } else {
                EvalCompleteness::Complete
            }
        });

        program.evaluate(&module_id, &eval).await?;
        drop(eval);
        let completeness = effects_task.await?;
        Ok(completeness)
    }
}

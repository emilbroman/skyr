use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    time::Duration,
};

use cdb::{DeploymentClient, DeploymentState};
use clap::Parser;
use futures_util::{StreamExt, TryStreamExt, future};
use sclc::SourceRepo;
use slog::{Drain, Logger, debug, error, info, o};
use tokio::{
    sync::mpsc,
    sync::oneshot::{self, error::TryRecvError},
    task,
    time::{Instant, sleep_until},
};

#[derive(Parser)]
enum Program {
    Daemon {
        #[clap(long = "db-hostname", default_value = "localhost")]
        db_hostname: String,

        #[clap(long = "mq-hostname", default_value = "localhost")]
        mq_hostname: String,
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
            db_hostname,
            mq_hostname,
        } => {
            info!(log, "starting deployment engine daemon");

            let cdb_client = cdb::ClientBuilder::new()
                .known_node(&db_hostname)
                .build()
                .await?;

            let rdb_client = rdb::ClientBuilder::new()
                .known_node(&db_hostname)
                .build()
                .await?;

            let rtq_publisher = rtq::ClientBuilder::new()
                .uri(format!("amqp://{}:5672/%2f", mq_hostname))
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

            let worker = Worker {
                log: log.new(o!("dep" => deployment.fqid())),
                client: client.repo(deployment.repository).deployment(deployment.id),
                namespace: rdb_client.namespace(namespace),
                rtq_publisher: rtq_publisher.clone(),
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EvalCompleteness {
    Complete,
    Partial,
    Unviable,
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
                        if let Some(superceded) = self.client.get_superceded().await? {
                            superceded.set(DeploymentState::Undesired).await?;
                        }
                    }
                    EvalCompleteness::Partial => {
                        info!(
                            &self.log,
                            "evaluation incomplete; deferring superceded deployment teardown"
                        );
                    }
                    EvalCompleteness::Unviable => {
                        info!(
                            &self.log,
                            "evaluation unviable; setting deployment DOWN and restoring superceded"
                        );
                        self.client.set(DeploymentState::Down).await?;
                        if let Some(superceded) = self.client.get_superceded().await? {
                            superceded.set(DeploymentState::Desired).await?;
                        }
                    }
                }

                Ok(())
            }

            DeploymentState::Undesired => {
                info!(&self.log, "tearing down");

                let owner = deployment.fqid();
                let owner_for_filter = owner.clone();
                let mut emitted = 0usize;
                let mut resources =
                    self.namespace
                        .list_resources()
                        .await?
                        .try_filter(move |resource| {
                            future::ready(
                                resource.owner.as_deref() == Some(owner_for_filter.as_str()),
                            )
                        });
                while let Some(resource) = resources.try_next().await? {
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
                        "type" => resource.resource_type,
                        "id" => resource.id,
                        "owner" => resource.owner
                    );
                }

                if emitted > 0 {
                    info!(&self.log, "queued {} destroy messages", emitted);
                    return Ok(());
                }

                info!(&self.log, "no more resources, setting state to DOWN");
                self.client.set(DeploymentState::Down).await?;
                Ok(())
            }

            DeploymentState::Lingering => {
                info!(&self.log, "lingering...");
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
        if diagnosed.diags().has_errors() {
            info!(&self.log, "compile produced errors; skipping evaluation");
            return Ok(EvalCompleteness::Unviable);
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
        let mut eval = sclc::Eval::new::<DeploymentClient>(effects_tx);
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
                },
            );
        }
        drop(resources);

        let log = self.log.clone();
        let namespace_id = self.namespace.namespace().to_owned();
        let rtq_publisher = self.rtq_publisher.clone();
        let effects_task = task::spawn(async move {
            let mut had_effect = false;
            while let Some(effect) = effects_rx.recv().await {
                had_effect = true;
                match effect {
                    sclc::Effect::CreateResource { id, inputs } => {
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
                            dependencies: vec![],
                        });
                        if let Err(error) = rtq_publisher.enqueue(&message).await {
                            error!(log, "failed to publish create message";
                                "error" => error.to_string()
                            );
                            continue;
                        }

                        info!(log, "effect create resource";
                            "type" => id.ty,
                            "id" => id.id,
                            "inputs" => format!("{:?}", inputs)
                        )
                    }
                    sclc::Effect::UpdateResource { id, inputs } => {
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
                            })
                        };
                        if let Err(error) = rtq_publisher.enqueue(&message).await {
                            error!(log, "failed to publish update message";
                                "error" => error.to_string()
                            );
                            continue;
                        }

                        info!(log, "effect update resource";
                            "type" => id.ty,
                            "id" => id.id,
                            "inputs" => format!("{:?}", inputs)
                        )
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

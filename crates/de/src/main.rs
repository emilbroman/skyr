use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    time::Duration,
};

use cdb::{DeploymentClient, DeploymentState};
use clap::Parser;
use futures_util::{StreamExt, TryStreamExt};
use sclc::SourceRepo;
use tokio::{
    sync::mpsc,
    sync::oneshot::{self, error::TryRecvError},
    task,
    time::{Instant, sleep_until},
};
use tracing::Instrument;

fn map_dependencies(namespace: &str, deps: Vec<ids::ResourceId>) -> Vec<rtq::ResourceRef> {
    deps.into_iter()
        .map(|dep| rtq::ResourceRef {
            environment_qid: namespace.to_owned(),
            resource_type: dep.typ,
            resource_name: dep.name,
        })
        .collect()
}

fn resource_ref(namespace: &str, id: &ids::ResourceId) -> rtq::ResourceRef {
    rtq::ResourceRef {
        environment_qid: namespace.to_owned(),
        resource_type: id.typ.clone(),
        resource_name: id.name.clone(),
    }
}

fn serialize_inputs(
    id: &ids::ResourceId,
    inputs: &sclc::Record,
    context: &str,
) -> Option<serde_json::Value> {
    match serde_json::to_value(inputs) {
        Ok(value) => Some(value),
        Err(error) => {
            tracing::error!(
                resource_type = %id.typ,
                resource_name = %id.name,
                error = %error,
                "failed to encode {context} inputs",
            );
            None
        }
    }
}

fn extract_deployment_id(owner_qid: String) -> String {
    owner_qid
        .rsplit_once('@')
        .map(|(_, id)| id.to_string())
        .unwrap_or(owner_qid)
}

fn diag_severity(level: sclc::DiagLevel) -> ldb::Severity {
    match level {
        sclc::DiagLevel::Error => ldb::Severity::Error,
        sclc::DiagLevel::Warning => ldb::Severity::Warning,
    }
}

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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    match Program::parse() {
        Program::Daemon {
            cdb_hostname,
            rdb_hostname,
            rtq_hostname,
            ldb_hostname,
        } => {
            tracing::info!("starting deployment engine daemon");

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
                    cdb_client.clone(),
                    rdb_client.clone(),
                    rtq_publisher.clone(),
                    ldb_publisher.clone(),
                    &mut workers,
                )
                .await
                {
                    tracing::error!("{e}")
                }

                tracing::debug!(
                    "will poll for new deployments again in {:.2}s",
                    (next_loop - Instant::now()).as_secs_f64()
                );
                sleep_until(next_loop).await;
            }
        }
    }
}

async fn process(
    client: cdb::Client,
    rdb_client: rdb::Client,
    rtq_publisher: rtq::Publisher,
    ldb_publisher: ldb::Publisher,
    workers: &mut BTreeMap<String, oneshot::Sender<()>>,
) -> anyhow::Result<()> {
    let deployments = client.active_deployments().await?.collect::<Vec<_>>().await;

    tracing::debug!("found {} deployments", deployments.len());

    let mut untouched = workers.keys().cloned().collect::<BTreeSet<_>>();
    for deployment in deployments {
        let deployment = deployment?;
        let deployment_qid = deployment.deployment_qid().to_string();
        if !untouched.remove(&deployment_qid) {
            tracing::debug!(dep = %deployment_qid, "new deployment to process");

            let (tx, rx) = oneshot::channel();
            // Resources are namespaced by environment QID.
            let environment_qid = deployment.environment_qid().to_string();

            let log_publisher = match ldb_publisher.namespace(deployment_qid.clone()).await {
                Ok(log_publisher) => log_publisher,
                Err(error) => {
                    tracing::error!(
                        dep = %deployment_qid,
                        error = %error,
                        "failed to create deployment log publisher topic",
                    );
                    continue;
                }
            };

            let worker = Worker {
                client: client.repo(deployment.repo.clone()).deployment(
                    deployment.environment.clone(),
                    deployment.deployment.clone(),
                ),
                namespace: rdb_client.namespace(environment_qid),
                rtq_publisher: rtq_publisher.clone(),
                log_publisher,
            };

            let span = tracing::info_span!("worker", dep = %deployment_qid);
            task::spawn(worker.run_loop(rx).instrument(span));

            workers.insert(deployment_qid, tx);
        }
    }

    for id in untouched {
        tracing::debug!(dep = %id, "no longer watching");
        workers.remove(&id);
    }

    Ok(())
}

struct Worker {
    client: DeploymentClient,
    namespace: rdb::NamespaceClient,
    rtq_publisher: rtq::Publisher,
    log_publisher: ldb::NamespacePublisher,
}

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
                        tracing::error!("{e}");
                    }
                }
            }

            tracing::debug!(
                "will reconcile in {:.2}s",
                (next_loop - Instant::now()).as_secs_f64()
            );
            sleep_until(next_loop).await;
        }
    }

    async fn work(&mut self) -> anyhow::Result<()> {
        let deployment = self.client.get().await?;
        let deployment_id = deployment.deployment.to_string();
        let short_id = &deployment_id[..8];

        match deployment.state {
            DeploymentState::Down => {
                tracing::info!("{short_id} down, waiting to be decommissioned...");
                Ok(())
            }

            DeploymentState::Desired => {
                tracing::info!("{short_id} reconciling");
                let completeness = self.compile_and_evaluate().await?;

                match completeness {
                    EvalCompleteness::Complete => {
                        for superseded in self.client.superseded().await? {
                            let superseded_deployment = superseded.get().await?;
                            if superseded_deployment.state == DeploymentState::Lingering {
                                superseded.set(DeploymentState::Undesired).await?;
                            }
                        }

                        // Check if all owned resources are non-volatile.
                        // If so, transition to Up (no further reconciliation needed).
                        let owner_deployment_qid = deployment.deployment_qid().to_string();
                        let mut has_volatile = false;
                        let mut resources = self
                            .namespace
                            .list_resources_by_owner(&owner_deployment_qid)
                            .await?;
                        while let Some(resource) = resources.try_next().await? {
                            if resource.markers.contains(&sclc::Marker::Volatile) {
                                has_volatile = true;
                                break;
                            }
                        }
                        drop(resources);

                        if !has_volatile {
                            tracing::info!(
                                "{short_id} all resources non-volatile; transitioning to UP"
                            );
                            self.client.set(DeploymentState::Up).await?;
                            self.log_publisher
                                .info(format!("{short_id} is up (all resources non-volatile)"))
                                .await;
                        }
                    }
                    EvalCompleteness::Partial => {
                        tracing::info!(
                            "evaluation incomplete; deferring superseded deployment teardown"
                        );
                    }
                }

                Ok(())
            }

            DeploymentState::Up => {
                tracing::debug!("{short_id} up; no reconciliation needed");
                Ok(())
            }

            DeploymentState::Undesired => {
                tracing::info!("{short_id} tearing down");

                let owner_deployment_qid = deployment.deployment_qid().to_string();
                let mut all_resources = Vec::new();
                let mut resources = self.namespace.list_resources().await?;
                while let Some(resource) = resources.try_next().await? {
                    all_resources.push(resource);
                }

                let owned_resources = all_resources
                    .iter()
                    .filter(|resource| {
                        resource.owner.as_deref() == Some(owner_deployment_qid.as_str())
                    })
                    .collect::<Vec<_>>();
                let mut emitted = 0usize;
                let mut blocked = 0usize;
                // Exclude dependencies from sticky resources owned by this
                // deployment so they don't block teardown of their own deps.
                let living_dependency_targets = all_resources
                    .iter()
                    .filter(|resource| {
                        !(resource.owner.as_deref() == Some(owner_deployment_qid.as_str())
                            && resource.markers.contains(&sclc::Marker::Sticky))
                    })
                    .flat_map(|resource| resource.dependencies.iter().cloned())
                    .collect::<HashSet<_>>();

                for resource in &owned_resources {
                    let resource_id = ids::ResourceId {
                        typ: resource.resource_type.clone(),
                        name: resource.name.clone(),
                    };

                    if resource.markers.contains(&sclc::Marker::Sticky) {
                        tracing::info!(
                            resource_type = %resource.resource_type,
                            resource_name = %resource.name,
                            "sticky resource; skipping destroy",
                        );
                        continue;
                    }

                    if living_dependency_targets.contains(&resource_id) {
                        blocked += 1;
                        tracing::info!(
                            resource_type = %resource.resource_type,
                            resource_name = %resource.name,
                            owner = ?resource.owner,
                            "resource still has living dependents; deferring destroy",
                        );
                        continue;
                    }

                    let message = rtq::Message::Destroy(rtq::DestroyMessage {
                        resource: resource_ref(self.namespace.namespace(), &resource_id),
                        deployment_id: deployment.deployment.to_string(),
                    });
                    self.rtq_publisher.enqueue(&message).await?;
                    emitted += 1;

                    tracing::info!(
                        resource_type = %resource.resource_type,
                        resource_name = %resource.name,
                        owner = ?resource.owner,
                        "queued destroy",
                    );
                }

                if emitted > 0 {
                    tracing::info!("queued {} destroy messages", emitted);
                    return Ok(());
                }

                let has_non_sticky = owned_resources
                    .iter()
                    .any(|r| !r.markers.contains(&sclc::Marker::Sticky));

                if !has_non_sticky {
                    for resource in &owned_resources {
                        self.log_publisher
                            .info(format!(
                                "{} will stick around",
                                ids::ResourceId::new(&resource.resource_type, &resource.name)
                            ))
                            .await;
                    }
                    tracing::info!(
                        "{short_id} no more non-sticky resources, setting state to DOWN"
                    );
                    self.client.set(DeploymentState::Down).await?;
                    self.log_publisher
                        .info(format!("Undesired {short_id} is fully torn down"))
                        .await;
                    return Ok(());
                }

                if blocked > 0 {
                    tracing::info!(
                        blocked_resources = blocked,
                        "{short_id} teardown waiting on living dependents",
                    );
                    self.log_publisher
                        .info(format!(
                            "Undesired {short_id} still has {blocked} resources with living dependents"
                        ))
                        .await;
                }
                Ok(())
            }

            DeploymentState::Lingering => {
                tracing::info!("{short_id} lingering...");
                let mut cursor = self.client.clone();
                let mut seen = HashSet::new();

                while let Some(superseding) = cursor.get_superseding().await? {
                    let superseding_deployment = superseding.get().await?;
                    let commit_hash = superseding_deployment.deployment.clone();

                    if !seen.insert(commit_hash) {
                        tracing::warn!("detected supersession cycle while lingering");
                        break;
                    }

                    if matches!(
                        superseding_deployment.state,
                        DeploymentState::Desired | DeploymentState::Up
                    ) {
                        self.client
                            .mark_superseded_by(&superseding_deployment.deployment)
                            .await?;
                        break;
                    }

                    cursor = superseding;
                }

                Ok(())
            }
        }
    }

    async fn publish_diagnostics(&self, diags: &sclc::DiagList) {
        for diag in diags.iter() {
            let (module_id, span) = diag.locate();
            tracing::info!(
                module = %module_id,
                span = %span,
                diag = %diag,
                "compile diagnostic",
            );
            self.log_publisher
                .log(
                    diag_severity(diag.level()),
                    format!("{module_id}:{span}: {diag}"),
                )
                .await
                .unwrap_or_default();
        }
    }

    async fn compile_and_evaluate(&mut self) -> anyhow::Result<EvalCompleteness> {
        let diagnosed = sclc::compile(self.client.clone()).await?;
        self.publish_diagnostics(diagnosed.diags()).await;

        if diagnosed.diags().has_errors() {
            tracing::info!("compile produced errors; skipping evaluation");
            return Ok(EvalCompleteness::Partial);
        }

        let mut program = diagnosed.into_inner();
        let module_id = SourceRepo::package_id(&self.client)
            .as_slice()
            .iter()
            .cloned()
            .chain(std::iter::once(String::from("Main")))
            .collect::<sclc::ModuleId>();
        let full_deployment_qid = self.client.deployment_qid();
        let owner_deployment_qid = full_deployment_qid.to_string();
        let deployment_id = full_deployment_qid.deployment.to_string();

        let (effects_tx, mut effects_rx) = mpsc::unbounded_channel();
        let mut eval =
            sclc::Eval::new::<DeploymentClient>(effects_tx, owner_deployment_qid.clone());
        let mut unowned_resource_owner_by_id = HashMap::new();
        let mut volatile_resource_ids = HashSet::new();
        let mut resources = self.namespace.list_resources().await?;
        while let Some(resource) = resources.try_next().await? {
            let resource_id = ids::ResourceId {
                typ: resource.resource_type.clone(),
                name: resource.name.clone(),
            };
            if resource.owner.as_deref() != Some(owner_deployment_qid.as_str())
                && let Some(owner) = resource.owner.clone()
            {
                unowned_resource_owner_by_id.insert(resource_id.clone(), owner);
            }
            if resource.markers.contains(&sclc::Marker::Volatile) {
                volatile_resource_ids.insert(resource_id.clone());
            }

            eval.add_resource(
                resource_id,
                sclc::Resource {
                    inputs: resource.inputs.unwrap_or_default(),
                    outputs: resource.outputs.unwrap_or_default(),
                    dependencies: resource.dependencies,
                    markers: resource.markers,
                },
            );
        }
        drop(resources);

        let log_publisher = self.log_publisher.clone();
        let namespace_id = self.namespace.namespace().to_owned();
        let rtq_publisher = self.rtq_publisher.clone();
        let effects_task = task::spawn(
            {
                async move {
                    let mut had_effect = false;
                    while let Some(effect) = effects_rx.recv().await {
                        match effect {
                            sclc::Effect::CreateResource {
                                id,
                                inputs,
                                dependencies,
                                source_trace,
                            } => {
                                had_effect = true;
                                let Some(inputs_value) = serialize_inputs(&id, &inputs, "create")
                                else {
                                    continue;
                                };
                                let message = rtq::Message::Create(rtq::CreateMessage {
                                    resource: resource_ref(&namespace_id, &id),
                                    deployment_id: deployment_id.clone(),
                                    inputs: inputs_value,
                                    dependencies: map_dependencies(&namespace_id, dependencies),
                                    source_trace,
                                });
                                if let Err(error) = rtq_publisher.enqueue(&message).await {
                                    tracing::error!(
                                        error = %error,
                                        "failed to publish create message",
                                    );

                                    log_publisher
                                        .error(format!("Failed to enqueue CREATE {id}: {error}",))
                                        .await;

                                    continue;
                                }

                                tracing::info!(
                                    resource_type = %id.typ,
                                    resource_name = %id.name,
                                    inputs = ?inputs,
                                    "effect create resource",
                                );
                            }
                            sclc::Effect::UpdateResource {
                                id,
                                inputs,
                                dependencies,
                                source_trace,
                            } => {
                                had_effect = true;
                                let Some(desired_inputs) = serialize_inputs(&id, &inputs, "update")
                                else {
                                    continue;
                                };
                                let dependencies = map_dependencies(&namespace_id, dependencies);
                                let message = if let Some(from_owner_qid) =
                                    unowned_resource_owner_by_id.get(&id).cloned()
                                {
                                    rtq::Message::Adopt(rtq::AdoptMessage {
                                        resource: resource_ref(&namespace_id, &id),
                                        from_deployment_id: extract_deployment_id(from_owner_qid),
                                        to_deployment_id: deployment_id.clone(),
                                        desired_inputs,
                                        dependencies,
                                        source_trace,
                                    })
                                } else {
                                    rtq::Message::Restore(rtq::RestoreMessage {
                                        resource: resource_ref(&namespace_id, &id),
                                        deployment_id: deployment_id.clone(),
                                        desired_inputs,
                                        dependencies,
                                        source_trace,
                                    })
                                };
                                if let Err(error) = rtq_publisher.enqueue(&message).await {
                                    tracing::error!(
                                        error = %error,
                                        "failed to publish update message",
                                    );

                                    log_publisher
                                        .error(format!("Failed to enqueue UPDATE {id}: {error}",))
                                        .await;
                                    continue;
                                }

                                tracing::info!(
                                    resource_type = %id.typ,
                                    resource_name = %id.name,
                                    inputs = ?inputs,
                                    "effect update resource",
                                );
                            }
                            sclc::Effect::TouchResource {
                                id,
                                inputs,
                                dependencies,
                                source_trace,
                            } => {
                                if let Some(from_owner_deployment_qid) =
                                    unowned_resource_owner_by_id.get(&id).cloned()
                                {
                                    had_effect = true;
                                    let Some(desired_inputs) =
                                        serialize_inputs(&id, &inputs, "touch")
                                    else {
                                        continue;
                                    };
                                    let message = rtq::Message::Adopt(rtq::AdoptMessage {
                                        resource: resource_ref(&namespace_id, &id),
                                        from_deployment_id: extract_deployment_id(
                                            from_owner_deployment_qid,
                                        ),
                                        to_deployment_id: deployment_id.clone(),
                                        desired_inputs,
                                        dependencies: map_dependencies(&namespace_id, dependencies),
                                        source_trace,
                                    });
                                    if let Err(error) = rtq_publisher.enqueue(&message).await {
                                        tracing::error!(
                                            error = %error,
                                            "failed to publish touch adopt message",
                                        );

                                        log_publisher
                                            .error(
                                                format!("Failed to enqueue ADOPT {id}: {error}",),
                                            )
                                            .await;
                                        continue;
                                    }

                                    tracing::info!(
                                        resource_type = %id.typ,
                                        resource_name = %id.name,
                                        inputs = ?inputs,
                                        "effect touch resource adopt",
                                    );
                                } else if volatile_resource_ids.contains(&id) {
                                    had_effect = true;
                                    let message = rtq::Message::Check(rtq::CheckMessage {
                                        resource: resource_ref(&namespace_id, &id),
                                        deployment_id: deployment_id.clone(),
                                    });
                                    if let Err(error) = rtq_publisher.enqueue(&message).await {
                                        tracing::error!(
                                            error = %error,
                                            "failed to publish touch check message",
                                        );

                                        log_publisher
                                            .error(
                                                format!("Failed to enqueue CHECK {id}: {error}",),
                                            )
                                            .await;
                                        continue;
                                    }

                                    tracing::info!(
                                        resource_type = %id.typ,
                                        resource_name = %id.name,
                                        "effect touch resource check",
                                    );
                                }
                            }
                        }
                    }

                    if had_effect {
                        EvalCompleteness::Partial
                    } else {
                        EvalCompleteness::Complete
                    }
                }
            }
            .instrument(tracing::Span::current()),
        );

        match program.evaluate(&module_id, &eval).await {
            Ok(eval_diagnosed) => {
                for diag in eval_diagnosed.diags().iter() {
                    let (module_id, span) = diag.locate();
                    self.log_publisher
                        .log(
                            diag_severity(diag.level()),
                            format!("{module_id}:{span}: {diag}"),
                        )
                        .await
                        .unwrap_or_default();
                }
            }
            Err(e) => {
                self.log_publisher.error(format!("{e}")).await;
            }
        }
        drop(eval);
        let completeness = effects_task.await?;
        Ok(completeness)
    }
}

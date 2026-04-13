use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use cdb::{DeploymentClient, DeploymentState};
use clap::Parser;
use futures_util::{StreamExt, TryStreamExt};
use tokio::{
    sync::mpsc,
    sync::oneshot::{self, error::TryRecvError},
    task,
    time::{Instant, sleep_until},
};
use tracing::Instrument;

fn map_dependencies(
    environment_qid: &ids::EnvironmentQid,
    deps: Vec<ids::ResourceId>,
) -> Vec<rtq::ResourceRef> {
    deps.into_iter()
        .map(|dep| rtq::ResourceRef {
            environment_qid: environment_qid.clone(),
            resource_id: dep,
        })
        .collect()
}

fn resource_ref(environment_qid: &ids::EnvironmentQid, id: &ids::ResourceId) -> rtq::ResourceRef {
    rtq::ResourceRef {
        environment_qid: environment_qid.clone(),
        resource_id: id.clone(),
    }
}

fn serialize_inputs(
    id: &ids::ResourceId,
    inputs: &sclc::Record,
    context: &str,
) -> anyhow::Result<serde_json::Value> {
    serde_json::to_value(inputs).map_err(|error| {
        anyhow::anyhow!(
            "failed to encode {context} inputs for {}:{}: {error}",
            id.typ,
            id.name,
        )
    })
}

/// Returns the deployment ID portion of an owner QID string, validating
/// that it parses as a well-formed `DeploymentQid`.
fn extract_deployment_id(owner_qid: &str) -> anyhow::Result<ids::DeploymentId> {
    let qid: ids::DeploymentQid = owner_qid
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid owner QID: {owner_qid}"))?;
    Ok(qid.deployment)
}

/// Returns a short (up to 8 char) prefix of the deployment ID for log messages.
fn short_id(deployment_id: &str) -> &str {
    deployment_id.get(..8).unwrap_or(deployment_id)
}

fn diag_severity(level: sclc::DiagLevel) -> ldb::Severity {
    match level {
        sclc::DiagLevel::Error => ldb::Severity::Error,
        sclc::DiagLevel::Warning => ldb::Severity::Warning,
    }
}

fn resource_id_from(resource: &rdb::Resource) -> ids::ResourceId {
    ids::ResourceId {
        typ: resource.resource_type.clone(),
        name: resource.name.clone(),
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

            let env_qid = deployment.environment_qid();
            let worker = Worker {
                client: client.repo(deployment.repo.clone()).deployment(
                    deployment.environment.clone(),
                    deployment.deployment.clone(),
                ),
                cdb_client: client.clone(),
                environment_qid: env_qid.clone(),
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
    cdb_client: cdb::Client,
    environment_qid: ids::EnvironmentQid,
    namespace: rdb::NamespaceClient,
    rtq_publisher: rtq::Publisher,
    log_publisher: ldb::NamespacePublisher,
}

enum EvalCompleteness {
    Complete,
    Partial,
}

struct EvalOutcome {
    completeness: EvalCompleteness,
    /// Resource IDs referenced during evaluation. When `fully_explored` is
    /// true, this is the complete set — any owned resource NOT in this set
    /// is no longer desired and should be destroyed.
    touched_resource_ids: HashSet<ids::ResourceId>,
    /// True when no Create/Update effects were emitted, meaning every
    /// resource function returned concrete outputs and all code paths
    /// were fully evaluated.
    fully_explored: bool,
}

/// Enqueue an RTQ message, logging errors to both tracing and the deployment
/// log publisher. Returns `true` if the message was enqueued successfully.
async fn enqueue_message(
    rtq_publisher: &rtq::Publisher,
    log_publisher: &ldb::NamespacePublisher,
    message: &rtq::Message,
    context: &str,
    id: &ids::ResourceId,
) -> bool {
    if let Err(error) = rtq_publisher.enqueue(message).await {
        tracing::error!(
            resource_type = %id.typ,
            resource_name = %id.name,
            error = %error,
            "failed to publish {context} message",
        );
        log_publisher
            .error(format!("Failed to enqueue {context} {id}: {error}",))
            .await;
        return false;
    }
    true
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
        let deployment_id = deployment.deployment.clone();
        let sid = short_id(deployment_id.as_str());

        match deployment.state {
            DeploymentState::Down => {
                tracing::info!("{sid} down, waiting to be decommissioned...");
                Ok(())
            }

            DeploymentState::Desired => {
                tracing::info!("{sid} reconciling");

                // If the commit has no Main.scl, there is nothing to
                // evaluate. Log an info note and transition directly to
                // Up instead of treating it as a compilation error.
                if self
                    .client
                    .path_hash(std::path::Path::new("Main.scl"))
                    .await?
                    .is_none()
                {
                    tracing::info!("{sid} no Main.scl; transitioning to UP");
                    self.log_publisher
                        .info(format!("{sid} is up (no Main.scl in commit)"))
                        .await;
                    self.client.set(DeploymentState::Up).await?;
                    return Ok(());
                }

                let outcome = self.compile_and_evaluate().await?;
                let owner_deployment_qid = deployment.deployment_qid().to_string();

                // When the evaluation tree was fully explored (no pending
                // Create/Update effects), we know the complete set of
                // referenced resources. Destroy any owned resources that
                // the evaluation never touched.
                if outcome.fully_explored {
                    self.destroy_untouched_resources(
                        &owner_deployment_qid,
                        &deployment_id,
                        &outcome.touched_resource_ids,
                    )
                    .await?;
                }

                match outcome.completeness {
                    EvalCompleteness::Complete => {
                        for superseded in self.client.superseded().await? {
                            let superseded_deployment = superseded.get().await?;
                            if superseded_deployment.state == DeploymentState::Lingering {
                                superseded.set(DeploymentState::Undesired).await?;
                            }
                        }

                        // Check if all owned resources are non-volatile.
                        // If so, transition to Up (no further reconciliation needed).
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
                            tracing::info!("{sid} all resources non-volatile; transitioning to UP");
                            self.client.set(DeploymentState::Up).await?;
                            self.log_publisher
                                .info(format!("{sid} is up (all resources non-volatile)"))
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
                tracing::debug!("{sid} up; no reconciliation needed");
                Ok(())
            }

            DeploymentState::Undesired => {
                tracing::info!("{sid} tearing down");

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
                    let resource_id = resource_id_from(resource);

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
                        resource: resource_ref(&self.environment_qid, &resource_id),
                        deployment_id: deployment.deployment.clone(),
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
                    tracing::info!("{sid} no more non-sticky resources, setting state to DOWN");
                    self.client.set(DeploymentState::Down).await?;
                    self.log_publisher
                        .info(format!("Undesired {sid} is fully torn down"))
                        .await;
                    return Ok(());
                }

                if blocked > 0 {
                    tracing::info!(
                        blocked_resources = blocked,
                        "{sid} teardown waiting on living dependents",
                    );
                    self.log_publisher
                        .info(format!(
                            "Undesired {sid} still has {blocked} resources with living dependents"
                        ))
                        .await;
                }
                Ok(())
            }

            DeploymentState::Lingering => {
                tracing::info!("{sid} lingering...");
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

                        // If the superseding deployment is already Up, it won't
                        // re-check its superseded list, so transition ourselves.
                        if superseding_deployment.state == DeploymentState::Up {
                            self.client.set(DeploymentState::Undesired).await?;
                        }

                        break;
                    }

                    cursor = superseding;
                }

                Ok(())
            }
        }
    }

    /// Destroy resources owned by this deployment that were not referenced
    /// during the latest evaluation. This handles the case where volatile
    /// resources change in a way that causes previously-created resources
    /// to disappear from the configuration.
    async fn destroy_untouched_resources(
        &self,
        owner_deployment_qid: &str,
        deployment_id: &ids::DeploymentId,
        touched_resource_ids: &HashSet<ids::ResourceId>,
    ) -> anyhow::Result<()> {
        let mut all_resources = Vec::new();
        let mut resources = self.namespace.list_resources().await?;
        while let Some(resource) = resources.try_next().await? {
            all_resources.push(resource);
        }
        drop(resources);

        let untouched_owned: Vec<_> = all_resources
            .iter()
            .filter(|resource| {
                resource.owner.as_deref() == Some(owner_deployment_qid)
                    && !touched_resource_ids.contains(&resource_id_from(resource))
            })
            .collect();

        if untouched_owned.is_empty() {
            return Ok(());
        }

        // Collect dependency targets from all living resources (excluding
        // untouched resources themselves) to avoid destroying resources
        // that are still depended upon.
        let living_dependency_targets: HashSet<ids::ResourceId> = all_resources
            .iter()
            .filter(|resource| {
                let id = resource_id_from(resource);
                // Don't let untouched owned resources block each other.
                resource.owner.as_deref() != Some(owner_deployment_qid)
                    || touched_resource_ids.contains(&id)
            })
            .flat_map(|resource| resource.dependencies.iter().cloned())
            .collect();

        for resource in &untouched_owned {
            let resource_id = resource_id_from(resource);

            if resource.markers.contains(&sclc::Marker::Sticky) {
                tracing::info!(
                    resource_type = %resource.resource_type,
                    resource_name = %resource.name,
                    "untouched sticky resource; skipping destroy",
                );
                continue;
            }

            if living_dependency_targets.contains(&resource_id) {
                tracing::info!(
                    resource_type = %resource.resource_type,
                    resource_name = %resource.name,
                    "untouched resource still has living dependents; deferring destroy",
                );
                continue;
            }

            let message = rtq::Message::Destroy(rtq::DestroyMessage {
                resource: resource_ref(&self.environment_qid, &resource_id),
                deployment_id: deployment_id.clone(),
            });
            self.rtq_publisher.enqueue(&message).await?;

            tracing::info!(
                resource_type = %resource.resource_type,
                resource_name = %resource.name,
                "queued destroy for untouched resource",
            );

            self.log_publisher
                .info(format!("Destroying untouched resource {resource_id}"))
                .await;
        }

        Ok(())
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
            if let Err(error) = self
                .log_publisher
                .log(
                    diag_severity(diag.level()),
                    format!("{module_id}:{span}: {diag}"),
                )
                .await
            {
                tracing::warn!(
                    error = %error,
                    "failed to publish diagnostic to log",
                );
            }
        }
    }

    async fn compile_and_evaluate(&mut self) -> anyhow::Result<EvalOutcome> {
        let user_pkg: Arc<dyn sclc::Package> = Arc::new(self.client.clone());
        let finder = sclc::build_cdb_finder(
            user_pkg,
            self.cdb_client.clone(),
            self.environment_qid.environment.clone(),
        );
        let repo_qid = self.client.repo_qid();
        let entry = [repo_qid.org.as_str(), repo_qid.repo.as_str(), "Main"];

        let diagnosed = sclc::compile(finder, &entry).await?;
        self.publish_diagnostics(diagnosed.diags()).await;

        if diagnosed.diags().has_errors() {
            tracing::info!("compile produced errors; skipping evaluation");
            return Ok(EvalOutcome {
                completeness: EvalCompleteness::Partial,
                touched_resource_ids: HashSet::new(),
                fully_explored: false,
            });
        }

        let asg = diagnosed.into_inner();
        let full_deployment_qid = self.client.deployment_qid();
        let owner_deployment_qid = full_deployment_qid.to_string();
        let deployment_id = full_deployment_qid.deployment.clone();

        // Collect the set of superseded deployment QIDs so we can validate
        // that adoption only happens from deployments we legitimately supersede.
        let mut superseded_deployment_qids = HashSet::new();
        for superseded in self.client.superseded().await? {
            let dep = superseded.get().await?;
            superseded_deployment_qids.insert(dep.deployment_qid().to_string());
        }

        let (effects_tx, mut effects_rx) = mpsc::unbounded_channel();
        let environment_qid_str = self.environment_qid.to_string();
        let mut eval_ctx = sclc::EvalCtx::new(effects_tx, environment_qid_str);
        let mut unowned_resource_owner_by_id = HashMap::new();
        let mut volatile_resource_ids = HashSet::new();
        let mut resources = self.namespace.list_resources().await?;
        while let Some(resource) = resources.try_next().await? {
            let resource_id = resource_id_from(&resource);
            if resource.owner.as_deref() != Some(owner_deployment_qid.as_str())
                && let Some(owner) = resource.owner.clone()
            {
                unowned_resource_owner_by_id.insert(resource_id.clone(), owner);
            }
            if resource.markers.contains(&sclc::Marker::Volatile) {
                volatile_resource_ids.insert(resource_id.clone());
            }

            eval_ctx.add_resource(
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
        let env_qid = self.environment_qid.clone();
        let rtq_publisher = self.rtq_publisher.clone();
        let effects_task = task::spawn(
            {
                async move {
                    let mut had_effect = false;
                    let mut had_mutation = false;
                    let mut touched_resource_ids = HashSet::new();
                    while let Some(effect) = effects_rx.recv().await {
                        match effect {
                            sclc::Effect::CreateResource {
                                id,
                                inputs,
                                dependencies,
                                source_trace,
                            } => {
                                had_effect = true;
                                had_mutation = true;
                                touched_resource_ids.insert(id.clone());
                                let inputs_value = match serialize_inputs(&id, &inputs, "create") {
                                    Ok(v) => v,
                                    Err(error) => {
                                        tracing::error!("{error:#}");
                                        log_publisher
                                            .error(format!("Skipping CREATE {id}: {error}"))
                                            .await;
                                        continue;
                                    }
                                };
                                let message = rtq::Message::Create(rtq::CreateMessage {
                                    resource: resource_ref(&env_qid, &id),
                                    deployment_id: deployment_id.clone(),
                                    inputs: inputs_value,
                                    dependencies: map_dependencies(&env_qid, dependencies),
                                    source_trace,
                                });
                                if !enqueue_message(
                                    &rtq_publisher,
                                    &log_publisher,
                                    &message,
                                    "CREATE",
                                    &id,
                                )
                                .await
                                {
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
                                had_mutation = true;
                                touched_resource_ids.insert(id.clone());
                                let desired_inputs =
                                    match serialize_inputs(&id, &inputs, "update") {
                                        Ok(v) => v,
                                        Err(error) => {
                                            tracing::error!("{error:#}");
                                            log_publisher
                                                .error(format!("Skipping UPDATE {id}: {error}"))
                                                .await;
                                            continue;
                                        }
                                    };
                                let dependencies = map_dependencies(&env_qid, dependencies);
                                let message = if let Some(from_owner_qid) =
                                    unowned_resource_owner_by_id.get(&id).cloned()
                                {
                                    // Validate that we are only adopting from a
                                    // superseded deployment.
                                    if !superseded_deployment_qids.contains(&from_owner_qid) {
                                        tracing::warn!(
                                            resource_type = %id.typ,
                                            resource_name = %id.name,
                                            from_owner = %from_owner_qid,
                                            "refusing to adopt resource from non-superseded deployment",
                                        );
                                        log_publisher
                                            .error(format!(
                                                "Cannot adopt {id}: owner {from_owner_qid} is not a superseded deployment",
                                            ))
                                            .await;
                                        continue;
                                    }
                                    let from_deployment_id =
                                        match extract_deployment_id(&from_owner_qid) {
                                            Ok(id) => id,
                                            Err(error) => {
                                                tracing::error!(
                                                    from_owner = %from_owner_qid,
                                                    "{error:#}",
                                                );
                                                continue;
                                            }
                                        };
                                    rtq::Message::Adopt(rtq::AdoptMessage {
                                        resource: resource_ref(&env_qid, &id),
                                        from_deployment_id,
                                        to_deployment_id: deployment_id.clone(),
                                        desired_inputs,
                                        dependencies,
                                        source_trace,
                                    })
                                } else {
                                    rtq::Message::Restore(rtq::RestoreMessage {
                                        resource: resource_ref(&env_qid, &id),
                                        deployment_id: deployment_id.clone(),
                                        desired_inputs,
                                        dependencies,
                                        source_trace,
                                    })
                                };
                                if !enqueue_message(
                                    &rtq_publisher,
                                    &log_publisher,
                                    &message,
                                    "UPDATE",
                                    &id,
                                )
                                .await
                                {
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
                                touched_resource_ids.insert(id.clone());
                                if let Some(from_owner_deployment_qid) =
                                    unowned_resource_owner_by_id.get(&id).cloned()
                                {
                                    // Validate that we are only adopting from a
                                    // superseded deployment.
                                    if !superseded_deployment_qids
                                        .contains(&from_owner_deployment_qid)
                                    {
                                        tracing::warn!(
                                            resource_type = %id.typ,
                                            resource_name = %id.name,
                                            from_owner = %from_owner_deployment_qid,
                                            "refusing to adopt-touch resource from non-superseded deployment",
                                        );
                                        log_publisher
                                            .error(format!(
                                                "Cannot adopt {id}: owner {from_owner_deployment_qid} is not a superseded deployment",
                                            ))
                                            .await;
                                        continue;
                                    }
                                    had_effect = true;
                                    let desired_inputs =
                                        match serialize_inputs(&id, &inputs, "touch") {
                                            Ok(v) => v,
                                            Err(error) => {
                                                tracing::error!("{error:#}");
                                                log_publisher
                                                    .error(format!(
                                                        "Skipping ADOPT {id}: {error}"
                                                    ))
                                                    .await;
                                                continue;
                                            }
                                        };
                                    let from_deployment_id = match extract_deployment_id(
                                        &from_owner_deployment_qid,
                                    ) {
                                        Ok(id) => id,
                                        Err(error) => {
                                            tracing::error!(
                                                from_owner = %from_owner_deployment_qid,
                                                "{error:#}",
                                            );
                                            continue;
                                        }
                                    };
                                    let message = rtq::Message::Adopt(rtq::AdoptMessage {
                                        resource: resource_ref(&env_qid, &id),
                                        from_deployment_id,
                                        to_deployment_id: deployment_id.clone(),
                                        desired_inputs,
                                        dependencies: map_dependencies(&env_qid, dependencies),
                                        source_trace,
                                    });
                                    if !enqueue_message(
                                        &rtq_publisher,
                                        &log_publisher,
                                        &message,
                                        "ADOPT",
                                        &id,
                                    )
                                    .await
                                    {
                                        continue;
                                    }

                                    tracing::info!(
                                        resource_type = %id.typ,
                                        resource_name = %id.name,
                                        inputs = ?inputs,
                                        "effect touch resource adopt",
                                    );
                                } else if volatile_resource_ids.contains(&id) {
                                    // Volatile checks verify resource health but do not
                                    // count as effects that block completeness. This
                                    // allows supersession of prior deployments even when
                                    // volatile resources are present.
                                    let message = rtq::Message::Check(rtq::CheckMessage {
                                        resource: resource_ref(&env_qid, &id),
                                        deployment_id: deployment_id.clone(),
                                    });
                                    if !enqueue_message(
                                        &rtq_publisher,
                                        &log_publisher,
                                        &message,
                                        "CHECK",
                                        &id,
                                    )
                                    .await
                                    {
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

                    EvalOutcome {
                        completeness: if had_effect {
                            EvalCompleteness::Partial
                        } else {
                            EvalCompleteness::Complete
                        },
                        touched_resource_ids,
                        fully_explored: !had_mutation,
                    }
                }
            }
            .instrument(tracing::Span::current()),
        );

        if let Err(e) = sclc::eval(&asg, eval_ctx) {
            self.log_publisher.error(format!("{e}")).await;
        }
        let outcome = effects_task.await?;
        Ok(outcome)
    }
}

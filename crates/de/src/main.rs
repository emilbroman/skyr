use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use cdb::{Deployment, DeploymentClient, DeploymentState};
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

/// Parses an owner QID string and extracts its deployment ID and nonce.
fn extract_deployment_identity(
    owner_qid: &str,
) -> anyhow::Result<(ids::DeploymentId, ids::DeploymentNonce)> {
    let qid: ids::DeploymentQid = owner_qid
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid owner QID: {owner_qid}"))?;
    Ok((qid.deployment, qid.nonce))
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

        #[clap(long = "worker-index", default_value_t = 0)]
        worker_index: u16,

        #[clap(long = "worker-count", default_value_t = 1)]
        worker_count: u16,
    },
}

/// Determines whether the given deployment is owned by this worker.
///
/// Interprets the first 16 hex characters of the deployment (commit hash) ID
/// as a big-endian u64 and assigns it to a worker via modulo division.
/// When `worker_count` is 1 every deployment is owned.
fn deployment_owned_by_worker(
    deployment_id: &ids::DeploymentId,
    worker_index: u16,
    worker_count: u16,
) -> bool {
    if worker_count <= 1 {
        return true;
    }
    let hex_prefix = &deployment_id.as_str()[..16];
    let hash = u64::from_str_radix(hex_prefix, 16).unwrap_or(0);
    (hash % worker_count as u64) == worker_index as u64
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
            worker_index,
            worker_count,
        } => {
            if worker_count == 0 {
                anyhow::bail!("--worker-count must be at least 1");
            }
            if worker_index >= worker_count {
                anyhow::bail!("--worker-index must be less than --worker-count");
            }

            tracing::info!(
                worker_index,
                worker_count,
                "starting deployment engine daemon",
            );

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
                    worker_index,
                    worker_count,
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
    worker_index: u16,
    worker_count: u16,
) -> anyhow::Result<()> {
    let all_deployments = client.active_deployments().await?.collect::<Vec<_>>().await;

    let deployments: Vec<_> = all_deployments
        .into_iter()
        .filter(|d| match d {
            Ok(d) => deployment_owned_by_worker(&d.deployment, worker_index, worker_count),
            Err(_) => true, // propagate errors
        })
        .collect();

    tracing::debug!(
        "found {} deployments for this worker (index={}, count={})",
        deployments.len(),
        worker_index,
        worker_count,
    );

    // Remove workers whose loop has exited (receiver dropped).
    workers.retain(|id, tx| {
        if tx.is_closed() {
            tracing::debug!(dep = %id, "worker exited; removing from pool");
            false
        } else {
            true
        }
    });

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
                    deployment.nonce,
                ),
                cdb_client: client.clone(),
                rdb_client: rdb_client.clone(),
                environment_qid: env_qid.clone(),
                namespace: rdb_client.namespace(environment_qid),
                rtq_publisher: rtq_publisher.clone(),
                log_publisher,
                last_failure_at: None,
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
    rdb_client: rdb::Client,
    environment_qid: ids::EnvironmentQid,
    namespace: rdb::NamespaceClient,
    rtq_publisher: rtq::Publisher,
    log_publisher: ldb::NamespacePublisher,
    /// Tracks when the last failure occurred, used together with the
    /// persisted `failures` counter to compute exponential backoff.
    last_failure_at: Option<Instant>,
}

/// Initial backoff delay after the first failure.
const BACKOFF_INITIAL: Duration = Duration::from_secs(5);
/// Maximum backoff delay between reconciliation attempts.
const BACKOFF_MAX: Duration = Duration::from_secs(24 * 60 * 60);
/// Multiplicative growth factor per failed attempt.
const BACKOFF_FACTOR: f64 = 1.1;

/// Compute the backoff duration for the given number of consecutive failures.
fn backoff_duration(failures: u32) -> Duration {
    if failures == 0 {
        return Duration::ZERO;
    }
    let factor = BACKOFF_FACTOR.powi(failures.saturating_sub(1) as i32);
    let delay_secs = (BACKOFF_INITIAL.as_secs_f64() * factor).min(BACKOFF_MAX.as_secs_f64());
    Duration::from_secs_f64(delay_secs)
}

struct EvalOutcome {
    /// Resource IDs referenced during evaluation. When `fully_explored` is
    /// true, this is the complete set — any owned resource NOT in this set
    /// is no longer desired and should be destroyed.
    touched_resource_ids: HashSet<ids::ResourceId>,
    /// True when no Create/Update effects were emitted, meaning every
    /// resource function returned concrete outputs and all code paths
    /// were fully evaluated.
    fully_explored: bool,
    /// True when at least one effect was emitted (Create, Update, Adopt, etc).
    had_effect: bool,
    /// Whether the deployment's manifest declares any volatile (branch or
    /// tag) cross-repo pins. Preserved for observability.
    #[allow(dead_code)]
    has_volatile_cross_repo_pins: bool,
    /// True when compilation produced one or more `Error`-severity
    /// diagnostics.
    had_fatal_errors: bool,
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
                Err(TryRecvError::Empty) => match self.work().await {
                    Ok(true) => {}
                    Ok(false) => return,
                    Err(e) => tracing::error!("{e}"),
                },
            }

            tracing::debug!(
                "will reconcile in {:.2}s",
                (next_loop - Instant::now()).as_secs_f64()
            );
            sleep_until(next_loop).await;
        }
    }

    /// Returns `Ok(false)` to signal the loop to stop.
    async fn work(&mut self) -> anyhow::Result<bool> {
        let deployment = self.client.get().await?;
        let sid = short_id(deployment.deployment.as_str()).to_string();

        match deployment.state {
            DeploymentState::Down => {
                tracing::info!("{sid} down, waiting to be decommissioned...");
                Ok(true)
            }

            DeploymentState::Desired => self.run_desired(&deployment).await,

            DeploymentState::Lingering => {
                self.run_lingering(&deployment).await?;
                Ok(true)
            }

            DeploymentState::Undesired => {
                self.run_undesired(&deployment).await?;
                Ok(true)
            }
        }
    }

    /// DESIRED state handler.
    ///
    /// 1. Check backoff (based on persisted failures counter).
    /// 2. Check supersession — if superseded, transition to LINGERING.
    /// 3. Compile and evaluate.
    /// 4. On success: set bootstrapped, transition superseded deployments.
    /// 5. On error: increment failures counter.
    ///
    /// Returns `Ok(false)` to signal that the processing loop should stop
    /// (e.g., when there is no `Main.scl` and thus no volatile resources).
    async fn run_desired(&mut self, deployment: &Deployment) -> anyhow::Result<bool> {
        let sid = short_id(deployment.deployment.as_str()).to_string();

        // Backoff: if we have failures, check whether enough time has passed.
        if deployment.failures > 0 {
            let delay = backoff_duration(deployment.failures);
            if let Some(last_failure) = self.last_failure_at {
                let elapsed = last_failure.elapsed();
                if elapsed < delay {
                    tracing::debug!(
                        "{sid} backing off (failures={}, {:.0}s remaining)",
                        deployment.failures,
                        (delay - elapsed).as_secs_f64(),
                    );
                    return Ok(true);
                }
            }
            // If last_failure_at is None (e.g., after restart), proceed
            // immediately — the first attempt after restart is free.
        }

        // Supersession check: if this deployment has been superseded,
        // transition to LINGERING immediately.
        if self.client.get_superseding().await?.is_some() {
            tracing::info!("{sid} superseded; transitioning to LINGERING");
            self.log_publisher
                .info(format!("{sid} superseded; transitioning to LINGERING"))
                .await;
            self.client.set(DeploymentState::Lingering).await?;
            return Ok(true);
        }

        tracing::info!("{sid} reconciling");

        // If the commit has no Main.scl, there is nothing to evaluate.
        // Mark as bootstrapped and stop the processing loop — there are
        // no volatile resources to re-check.
        if self
            .client
            .path_hash(std::path::Path::new("Main.scl"))
            .await?
            .is_none()
        {
            tracing::info!("{sid} no Main.scl; marking bootstrapped");
            self.log_publisher
                .info(format!("{sid} bootstrapped (no Main.scl in commit)"))
                .await;
            self.client.set_progress(true, 0).await?;
            self.transition_superseded_to_undesired().await?;
            return Ok(false);
        }

        // Compile and evaluate.
        match self.compile_and_evaluate().await {
            Ok(outcome) => {
                if outcome.had_fatal_errors {
                    // Fatal compile errors: increment failures, stay DESIRED.
                    let new_failures = deployment.failures.saturating_add(1);
                    self.last_failure_at = Some(Instant::now());
                    self.client
                        .set_progress(deployment.bootstrapped, new_failures)
                        .await?;
                    tracing::warn!("{sid} fatal compile errors (failures={})", new_failures,);
                    self.log_publisher
                        .error(format!("{sid} compile errors (failures={new_failures})"))
                        .await;
                    return Ok(true);
                }

                // Success: reset failures.
                let owner_deployment_qid = deployment.deployment_qid().to_string();
                let deployment_id = deployment.deployment.clone();
                let deployment_nonce = deployment.nonce;

                if outcome.fully_explored {
                    self.destroy_untouched_resources(
                        &owner_deployment_qid,
                        &deployment_id,
                        deployment_nonce,
                        &outcome.touched_resource_ids,
                    )
                    .await?;
                }

                if !outcome.had_effect {
                    // Fully converged — set bootstrapped.
                    if !deployment.bootstrapped {
                        tracing::info!("{sid} bootstrapped");
                        self.log_publisher.info(format!("{sid} bootstrapped")).await;
                    }
                    self.client.set_progress(true, 0).await?;
                    self.transition_superseded_to_undesired().await?;
                } else {
                    // Partial evaluation — reset failures but not yet bootstrapped.
                    if deployment.failures > 0 {
                        self.client.set_progress(deployment.bootstrapped, 0).await?;
                    }
                    self.last_failure_at = None;
                    tracing::info!(
                        "{sid} evaluation incomplete; deferring superseded deployment teardown"
                    );
                }

                Ok(true)
            }
            Err(error) => {
                // Transient error: increment failures, stay DESIRED.
                let new_failures = deployment.failures.saturating_add(1);
                self.last_failure_at = Some(Instant::now());
                self.client
                    .set_progress(deployment.bootstrapped, new_failures)
                    .await?;
                tracing::warn!(
                    "{sid} transient error (failures={}): {error:#}",
                    new_failures,
                );
                self.log_publisher
                    .warn(format!("{sid} transient error: {error}"))
                    .await;
                Ok(true)
            }
        }
    }

    /// Transition all superseded DESIRED/LINGERING deployments to UNDESIRED.
    /// Called when this deployment becomes bootstrapped.
    ///
    /// DESIRED deployments are included because a no-Main.scl deployment
    /// may have stopped its processing loop while still in DESIRED state.
    async fn transition_superseded_to_undesired(&self) -> anyhow::Result<()> {
        for superseded in self.client.superseded().await? {
            let superseded_deployment = superseded.get().await?;
            if matches!(
                superseded_deployment.state,
                DeploymentState::Desired | DeploymentState::Lingering
            ) {
                superseded.set(DeploymentState::Undesired).await?;
            }
        }
        Ok(())
    }

    /// LINGERING state handler.
    ///
    /// Idle until Current (the unsuperseded deployment) is bootstrapped,
    /// then transition to UNDESIRED.
    async fn run_lingering(&self, deployment: &Deployment) -> anyhow::Result<()> {
        let sid = short_id(deployment.deployment.as_str()).to_string();
        tracing::info!("{sid} lingering...");

        // Follow the supersession chain to find Current.
        let mut cursor = self.client.clone();
        let mut seen = HashSet::new();

        while let Some(superseding) = cursor.get_superseding().await? {
            let superseding_deployment = superseding.get().await?;
            let commit_hash = superseding_deployment.deployment.clone();

            if !seen.insert((commit_hash.clone(), superseding_deployment.nonce)) {
                tracing::warn!("detected supersession cycle while lingering");
                break;
            }

            // Check if this is Current (unsuperseded).
            if superseding.get_superseding().await?.is_none() {
                // This is Current. Check if it's bootstrapped.
                if superseding_deployment.bootstrapped {
                    tracing::info!(
                        "{sid} current deployment is bootstrapped; transitioning to UNDESIRED"
                    );
                    self.client.set(DeploymentState::Undesired).await?;
                    self.log_publisher
                        .info(format!("{sid} current is bootstrapped; now UNDESIRED"))
                        .await;
                }
                break;
            }

            cursor = superseding;
        }

        Ok(())
    }

    /// UNDESIRED state handler.
    ///
    /// Destroy resources in Teardown(µ) — resources owned by this deployment
    /// that are no longer needed by Current. Transition to DOWN when complete.
    async fn run_undesired(&self, deployment: &Deployment) -> anyhow::Result<()> {
        let sid = short_id(deployment.deployment.as_str()).to_string();
        tracing::info!("{sid} tearing down");

        let owner_deployment_qid = deployment.deployment_qid().to_string();
        let mut all_resources = Vec::new();
        let mut resources = self.namespace.list_resources().await?;
        while let Some(resource) = resources.try_next().await? {
            all_resources.push(resource);
        }

        let owned_resources = all_resources
            .iter()
            .filter(|resource| resource.owner.as_deref() == Some(owner_deployment_qid.as_str()))
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
                deployment_nonce: deployment.nonce,
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

    /// Destroy resources owned by this deployment that were not referenced
    /// during the latest evaluation.
    async fn destroy_untouched_resources(
        &self,
        owner_deployment_qid: &str,
        deployment_id: &ids::DeploymentId,
        deployment_nonce: ids::DeploymentNonce,
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

        let living_dependency_targets: HashSet<ids::ResourceId> = all_resources
            .iter()
            .filter(|resource| {
                let id = resource_id_from(resource);
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
                deployment_nonce,
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
        let repo_qid = self.client.repo_qid().clone();

        let cross_repo_finder = self
            .build_cross_repo_finder(Arc::clone(&user_pkg), &repo_qid)
            .await?;

        let finder = build_full_finder(
            Arc::clone(&user_pkg),
            self.cdb_client.clone(),
            self.environment_qid.environment.clone(),
            cross_repo_finder.clone(),
        );

        let entry = [repo_qid.org.as_str(), repo_qid.repo.as_str(), "Main"];

        let diagnosed = sclc::compile(finder, &entry).await?;
        self.publish_diagnostics(diagnosed.diags()).await;

        if diagnosed.diags().has_errors() {
            tracing::info!("compile produced errors; skipping evaluation");
            return Ok(EvalOutcome {
                touched_resource_ids: HashSet::new(),
                fully_explored: false,
                had_effect: false,
                has_volatile_cross_repo_pins: cross_repo_finder
                    .as_ref()
                    .is_some_and(|f| f.has_volatile_pins()),
                had_fatal_errors: true,
            });
        }

        let asg = diagnosed.into_inner();
        let full_deployment_qid = self.client.deployment_qid();
        let owner_deployment_qid = full_deployment_qid.to_string();
        let deployment_id = full_deployment_qid.deployment.clone();
        let deployment_nonce = full_deployment_qid.nonce;

        let (effects_tx, mut effects_rx) = mpsc::unbounded_channel();
        let environment_qid_str = self.environment_qid.to_string();
        let local_deployment_qid = self.client.deployment_qid();
        let mut eval_ctx = sclc::EvalCtx::new(
            effects_tx,
            environment_qid_str,
            local_deployment_qid.clone(),
        );

        if let Some(finder) = &cross_repo_finder {
            for (foreign_repo, foreign_owner) in finder.resolved_owners().await {
                self.log_publisher
                    .info(format!(
                        "loaded foreign package {foreign_repo} -> {foreign_owner}"
                    ))
                    .await;
                let pkg_id = sclc::package_id_for_repo(&foreign_repo);
                eval_ctx.set_package_owner(pkg_id, foreign_owner.clone());

                let foreign_env_qid = foreign_owner.environment_qid().to_string();
                let foreign_owner_qid_str = foreign_owner.to_string();
                let foreign_namespace = self.rdb_client.namespace(foreign_env_qid);
                let mut foreign_resources = match foreign_namespace
                    .list_resources_by_owner(&foreign_owner_qid_str)
                    .await
                {
                    Ok(stream) => stream,
                    Err(e) => {
                        tracing::warn!(
                            owner = %foreign_owner,
                            "failed to load foreign resources: {e}",
                        );
                        continue;
                    }
                };
                while let Some(result) = foreign_resources.try_next().await? {
                    let id = resource_id_from(&result);
                    eval_ctx.add_foreign_resource(
                        foreign_owner.clone(),
                        id,
                        sclc::Resource {
                            inputs: result.inputs.unwrap_or_default(),
                            outputs: result.outputs.unwrap_or_default(),
                            dependencies: result.dependencies,
                            markers: result.markers,
                        },
                    );
                }
            }
        }
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
        let local_deployment_qid_for_drain = local_deployment_qid.clone();
        let has_volatile_cross_repo_pins = cross_repo_finder
            .as_ref()
            .is_some_and(|f| f.has_volatile_pins());
        let effects_task = task::spawn(
            {
                async move {
                    let mut had_effect = false;
                    let mut had_mutation = false;
                    let mut touched_resource_ids = HashSet::new();
                    while let Some(effect) = effects_rx.recv().await {
                        if effect.owner() != &local_deployment_qid_for_drain {
                            tracing::debug!(
                                owner = %effect.owner(),
                                local_owner = %local_deployment_qid_for_drain,
                                "dropping foreign-owned effect",
                            );
                            continue;
                        }
                        match effect {
                            sclc::Effect::CreateResource {
                                id,
                                inputs,
                                dependencies,
                                source_trace,
                                owner: _,
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
                                    deployment_nonce,
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
                                owner: _,
                            } => {
                                had_effect = true;
                                had_mutation = true;
                                touched_resource_ids.insert(id.clone());
                                let desired_inputs = match serialize_inputs(&id, &inputs, "update")
                                {
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
                                    let (from_deployment_id, from_deployment_nonce) =
                                        match extract_deployment_identity(&from_owner_qid) {
                                            Ok(v) => v,
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
                                        from_deployment_nonce,
                                        to_deployment_id: deployment_id.clone(),
                                        to_deployment_nonce: deployment_nonce,
                                        desired_inputs,
                                        dependencies,
                                        source_trace,
                                    })
                                } else {
                                    rtq::Message::Restore(rtq::RestoreMessage {
                                        resource: resource_ref(&env_qid, &id),
                                        deployment_id: deployment_id.clone(),
                                        deployment_nonce,
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
                                owner: _,
                            } => {
                                touched_resource_ids.insert(id.clone());
                                if let Some(from_owner_deployment_qid) =
                                    unowned_resource_owner_by_id.get(&id).cloned()
                                {
                                    had_effect = true;
                                    let desired_inputs =
                                        match serialize_inputs(&id, &inputs, "touch") {
                                            Ok(v) => v,
                                            Err(error) => {
                                                tracing::error!("{error:#}");
                                                log_publisher
                                                    .error(format!("Skipping ADOPT {id}: {error}"))
                                                    .await;
                                                continue;
                                            }
                                        };
                                    let (from_deployment_id, from_deployment_nonce) =
                                        match extract_deployment_identity(
                                            &from_owner_deployment_qid,
                                        ) {
                                            Ok(v) => v,
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
                                        from_deployment_nonce,
                                        to_deployment_id: deployment_id.clone(),
                                        to_deployment_nonce: deployment_nonce,
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
                                    let message = rtq::Message::Check(rtq::CheckMessage {
                                        resource: resource_ref(&env_qid, &id),
                                        deployment_id: deployment_id.clone(),
                                        deployment_nonce,
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
                        touched_resource_ids,
                        fully_explored: !had_mutation,
                        had_effect,
                        has_volatile_cross_repo_pins,
                        had_fatal_errors: false,
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

    /// Load the local repo's `Package.scle` (if any) and build a
    /// [`sclc::CrossRepoPackageFinder`] from its declared dependencies.
    /// Returns `Ok(None)` when the manifest is absent or has no deps.
    async fn build_cross_repo_finder(
        &self,
        user_pkg: Arc<dyn sclc::Package>,
        local_repo: &ids::RepoQid,
    ) -> anyhow::Result<Option<Arc<sclc::CrossRepoPackageFinder>>> {
        let manifest_finder = sclc::build_default_finder(Arc::clone(&user_pkg));
        let manifest = match sclc::load_manifest(Arc::clone(&user_pkg), manifest_finder).await {
            Ok(Some(m)) => m,
            Ok(None) => return Ok(None),
            Err(e) => {
                self.log_publisher
                    .error(format!("failed to load Package.scle: {e}"))
                    .await;
                return Err(anyhow::anyhow!("invalid Package.scle: {e}"));
            }
        };

        if manifest.dependencies.is_empty() {
            return Ok(None);
        }

        Ok(Some(Arc::new(sclc::CrossRepoPackageFinder::new(
            self.cdb_client.clone(),
            local_repo.org.clone(),
            manifest.dependencies,
        ))))
    }
}

/// Compose the finder chain used during compile: local user package →
/// cross-repo finder (if any) → CDB-backed cross-repo fallback for the
/// local environment → standard library.
fn build_full_finder(
    user_pkg: Arc<dyn sclc::Package>,
    cdb_client: cdb::Client,
    environment: ids::EnvironmentId,
    cross_repo_finder: Option<Arc<sclc::CrossRepoPackageFinder>>,
) -> Arc<dyn sclc::PackageFinder> {
    use sclc::CompositePackageFinder;

    let std_pkg: Arc<dyn sclc::Package> = Arc::new(sclc::StdPackage::new());
    let cdb_finder: Arc<dyn sclc::PackageFinder> =
        Arc::new(sclc::CdbPackageFinder::new(cdb_client, environment));

    let mut finders: Vec<Arc<dyn sclc::PackageFinder>> = Vec::new();
    finders.push(wrap_pkg(user_pkg));
    if let Some(cr) = cross_repo_finder {
        finders.push(cr);
    }
    finders.push(cdb_finder);
    finders.push(wrap_pkg(std_pkg));

    Arc::new(CompositePackageFinder::new(finders))
}

fn wrap_pkg(pkg: Arc<dyn sclc::Package>) -> Arc<dyn sclc::PackageFinder> {
    struct PkgFinder(Arc<dyn sclc::Package>);

    #[async_trait::async_trait]
    impl sclc::PackageFinder for PkgFinder {
        async fn find(
            &self,
            raw_id: &[&str],
        ) -> Result<Option<Arc<dyn sclc::Package>>, sclc::LoadError> {
            let pkg_id = self.0.id();
            let segments = pkg_id.as_slice();
            if raw_id.len() >= segments.len()
                && raw_id[..segments.len()]
                    .iter()
                    .zip(segments.iter())
                    .all(|(a, b)| *a == b.as_str())
            {
                Ok(Some(Arc::clone(&self.0)))
            } else {
                Ok(None)
            }
        }
    }

    Arc::new(PkgFinder(pkg))
}

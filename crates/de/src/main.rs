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
                rdb_client: rdb_client.clone(),
                environment_qid: env_qid.clone(),
                namespace: rdb_client.namespace(environment_qid),
                rtq_publisher: rtq_publisher.clone(),
                log_publisher,
                backoff: None,
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
    /// In-memory backoff state for `Failing` deployments. Reset whenever
    /// the deployment transitions out of `Failing` (by recovery or fatal
    /// escalation). Intentionally not persisted: on worker restart we start
    /// fresh at attempt 1, which is acceptable since backoff is an
    /// optimisation to reduce reconciliation pressure, not a correctness
    /// property.
    backoff: Option<BackoffState>,
}

/// Initial backoff delay after the first transient failure (`Desired` →
/// `Failing`).
const BACKOFF_INITIAL: Duration = Duration::from_secs(5);
/// Maximum backoff delay between reconciliation attempts for a `Failing`
/// deployment. Long, because a fatal-in-practice failure may require
/// operator work that is not worth hammering on; the operator can manually
/// nudge the deployment back to `Desired` once they've addressed the
/// underlying issue.
const BACKOFF_MAX: Duration = Duration::from_secs(24 * 60 * 60);
/// Multiplicative growth factor per failed attempt. With a 5s base and 1.1
/// growth, cumulative retry time reaches roughly a day after ~80 attempts.
const BACKOFF_FACTOR: f64 = 1.1;

#[derive(Clone, Copy, Debug)]
struct BackoffState {
    attempts: u32,
    next_attempt_at: Instant,
}

impl BackoffState {
    /// Compute the next backoff entry given the previous state (if any).
    fn bump(prev: Option<Self>, now: Instant) -> Self {
        let attempts = prev.map_or(0, |s| s.attempts).saturating_add(1);
        let factor = BACKOFF_FACTOR.powi(attempts.saturating_sub(1) as i32);
        let delay_secs = (BACKOFF_INITIAL.as_secs_f64() * factor).min(BACKOFF_MAX.as_secs_f64());
        Self {
            attempts,
            next_attempt_at: now + Duration::from_secs_f64(delay_secs),
        }
    }
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
    /// Whether the deployment's manifest declares any volatile (branch or
    /// tag) cross-repo pins. Such deployments stay in `Desired` and keep
    /// reconciling so the foreign upstream's changes propagate. See
    /// `CROSS_REPO_IMPORTS.md` §6a.
    has_volatile_cross_repo_pins: bool,
    /// True when compilation produced one or more `Error`-severity
    /// diagnostics. These are treated as *fatal* — retrying will not
    /// recover without a code change — so the deployment is escalated to
    /// `Failed` rather than kept in retry.
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

/// Validate that `from_owner_qid` is in the set of superseded deployments
/// and extract its deployment ID. Returns `None` (with logging) if
/// validation fails.
async fn validate_adoption(
    id: &ids::ResourceId,
    from_owner_qid: &str,
    superseded_deployment_qids: &HashSet<String>,
    log_publisher: &ldb::NamespacePublisher,
    context: &str,
) -> Option<ids::DeploymentId> {
    if !superseded_deployment_qids.contains(from_owner_qid) {
        tracing::warn!(
            resource_type = %id.typ,
            resource_name = %id.name,
            from_owner = %from_owner_qid,
            "refusing to {context} resource from non-superseded deployment",
        );
        log_publisher
            .error(format!(
                "Cannot adopt {id}: owner {from_owner_qid} is not a superseded deployment",
            ))
            .await;
        return None;
    }
    match extract_deployment_id(from_owner_qid) {
        Ok(deployment_id) => Some(deployment_id),
        Err(error) => {
            tracing::error!(
                from_owner = %from_owner_qid,
                "{error:#}",
            );
            None
        }
    }
}

/// Parameters for [`destroy_resources`].
struct DestroyParams<'a> {
    all_resources: &'a [rdb::Resource],
    environment_qid: &'a ids::EnvironmentQid,
    deployment_id: &'a ids::DeploymentId,
    rtq_publisher: &'a rtq::Publisher,
    log_publisher: &'a ldb::NamespacePublisher,
}

/// Outcome of a [`destroy_resources`] call.
struct DestroyOutcome {
    emitted: usize,
    blocked: usize,
    has_non_sticky: bool,
}

/// Destroy a set of owned resources, respecting sticky markers and living
/// dependents. Used by both `Undesired` teardown and untouched-resource
/// cleanup.
///
/// `exclude_from_living` is a predicate that returns `true` for resources
/// whose dependencies should NOT block teardown (e.g. sticky resources
/// owned by the same deployment during `Undesired` teardown, or untouched
/// owned resources during evaluation cleanup).
async fn destroy_resources<F>(
    owned_resources: &[&rdb::Resource],
    params: &DestroyParams<'_>,
    exclude_from_living: F,
) -> anyhow::Result<DestroyOutcome>
where
    F: Fn(&rdb::Resource) -> bool,
{
    let living_dependency_targets: HashSet<ids::ResourceId> = params
        .all_resources
        .iter()
        .filter(|resource| !exclude_from_living(resource))
        .flat_map(|resource| resource.dependencies.iter().cloned())
        .collect();

    let mut emitted = 0usize;
    let mut blocked = 0usize;
    let mut has_non_sticky = false;

    for resource in owned_resources {
        let resource_id = resource_id_from(resource);

        if resource.markers.contains(&sclc::Marker::Sticky) {
            tracing::info!(
                resource_type = %resource.resource_type,
                resource_name = %resource.name,
                "sticky resource; skipping destroy",
            );
            continue;
        }

        has_non_sticky = true;

        if living_dependency_targets.contains(&resource_id) {
            blocked += 1;
            tracing::info!(
                resource_type = %resource.resource_type,
                resource_name = %resource.name,
                "resource still has living dependents; deferring destroy",
            );
            continue;
        }

        let message = rtq::Message::Destroy(rtq::DestroyMessage {
            resource: resource_ref(params.environment_qid, &resource_id),
            deployment_id: params.deployment_id.clone(),
        });
        params.rtq_publisher.enqueue(&message).await?;
        emitted += 1;

        tracing::info!(
            resource_type = %resource.resource_type,
            resource_name = %resource.name,
            "queued destroy",
        );

        params
            .log_publisher
            .info(format!("Destroying {resource_id}"))
            .await;
    }

    Ok(DestroyOutcome {
        emitted,
        blocked,
        has_non_sticky,
    })
}

/// Drain the effects channel, dispatching Create/Update/Touch effects to
/// RTQ. Runs as a spawned task concurrently with evaluation.
async fn drain_effects(
    mut effects_rx: mpsc::UnboundedReceiver<sclc::Effect>,
    local_deployment_qid: ids::DeploymentQid,
    deployment_id: ids::DeploymentId,
    env_qid: ids::EnvironmentQid,
    rtq_publisher: rtq::Publisher,
    log_publisher: ldb::NamespacePublisher,
    unowned_resource_owner_by_id: HashMap<ids::ResourceId, String>,
    superseded_deployment_qids: HashSet<String>,
    volatile_resource_ids: HashSet<ids::ResourceId>,
    has_volatile_cross_repo_pins: bool,
) -> EvalOutcome {
    let mut had_effect = false;
    let mut had_mutation = false;
    let mut touched_resource_ids = HashSet::new();
    while let Some(effect) = effects_rx.recv().await {
        // Drop foreign-owned effects. Phase 3 will route
        // these through remote-state-read logic; for now we
        // simply log and skip.
        if effect.owner() != &local_deployment_qid {
            tracing::debug!(
                owner = %effect.owner(),
                local_owner = %local_deployment_qid,
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
                    inputs: inputs_value,
                    dependencies: map_dependencies(&env_qid, dependencies),
                    source_trace,
                });
                if !enqueue_message(&rtq_publisher, &log_publisher, &message, "CREATE", &id).await {
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
                let desired_inputs = match serialize_inputs(&id, &inputs, "update") {
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
                let message =
                    if let Some(from_owner_qid) = unowned_resource_owner_by_id.get(&id).cloned() {
                        let from_deployment_id = match validate_adoption(
                            &id,
                            &from_owner_qid,
                            &superseded_deployment_qids,
                            &log_publisher,
                            "adopt",
                        )
                        .await
                        {
                            Some(id) => id,
                            None => continue,
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
                if !enqueue_message(&rtq_publisher, &log_publisher, &message, "UPDATE", &id).await {
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
                if let Some(from_owner_qid) = unowned_resource_owner_by_id.get(&id).cloned() {
                    let from_deployment_id = match validate_adoption(
                        &id,
                        &from_owner_qid,
                        &superseded_deployment_qids,
                        &log_publisher,
                        "adopt-touch",
                    )
                    .await
                    {
                        Some(id) => id,
                        None => continue,
                    };
                    had_effect = true;
                    let desired_inputs = match serialize_inputs(&id, &inputs, "touch") {
                        Ok(v) => v,
                        Err(error) => {
                            tracing::error!("{error:#}");
                            log_publisher
                                .error(format!("Skipping ADOPT {id}: {error}"))
                                .await;
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
                    if !enqueue_message(&rtq_publisher, &log_publisher, &message, "ADOPT", &id)
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
                    if !enqueue_message(&rtq_publisher, &log_publisher, &message, "CHECK", &id)
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
        has_volatile_cross_repo_pins,
        had_fatal_errors: false,
    }
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
        let sid = short_id(deployment_id.as_str()).to_string();

        match deployment.state {
            DeploymentState::Down => {
                tracing::info!("{sid} down, waiting to be decommissioned...");
                Ok(())
            }

            DeploymentState::Failed => {
                tracing::debug!("{sid} failed; no reconciliation");
                Ok(())
            }

            DeploymentState::Desired => self.run_reconcile_active(&deployment).await,

            DeploymentState::Failing => {
                // Respect in-memory backoff: if the next attempt isn't due
                // yet, skip this iteration entirely.
                if let Some(state) = self.backoff {
                    let now = Instant::now();
                    if now < state.next_attempt_at {
                        tracing::debug!(
                            "{sid} failing (attempt {}); backoff in {:.0}s",
                            state.attempts,
                            (state.next_attempt_at - now).as_secs_f64(),
                        );
                        return Ok(());
                    }
                }
                self.run_reconcile_active(&deployment).await
            }

            DeploymentState::Up => {
                tracing::debug!("{sid} up; no reconciliation needed");
                Ok(())
            }

            DeploymentState::Undesired => {
                tracing::info!("{sid} tearing down");

                let owner_deployment_qid = deployment.deployment_qid().to_string();
                let all_resources = self.collect_all_resources().await?;

                let owned_resources: Vec<_> = all_resources
                    .iter()
                    .filter(|resource| {
                        resource.owner.as_deref() == Some(owner_deployment_qid.as_str())
                    })
                    .collect();

                let params = DestroyParams {
                    all_resources: &all_resources,
                    environment_qid: &self.environment_qid,
                    deployment_id: &deployment.deployment,
                    rtq_publisher: &self.rtq_publisher,
                    log_publisher: &self.log_publisher,
                };

                // Exclude dependencies from sticky resources owned by this
                // deployment so they don't block teardown of their own deps.
                let outcome = destroy_resources(&owned_resources, &params, |resource| {
                    resource.owner.as_deref() == Some(owner_deployment_qid.as_str())
                        && resource.markers.contains(&sclc::Marker::Sticky)
                })
                .await?;

                if outcome.emitted > 0 {
                    tracing::info!("queued {} destroy messages", outcome.emitted);
                    return Ok(());
                }

                if !outcome.has_non_sticky {
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

                if outcome.blocked > 0 {
                    tracing::info!(
                        blocked_resources = outcome.blocked,
                        "{sid} teardown waiting on living dependents",
                    );
                    self.log_publisher
                        .info(format!(
                            "Undesired {sid} still has {} resources with living dependents",
                            outcome.blocked,
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

    /// Dispatch to [`reconcile_active`](Self::reconcile_active) and apply
    /// the resulting state transition.
    ///
    /// * Ok with fatal errors → transition to `Failed`, clear backoff, and
    ///   try to make the deployment this one superseded desired again
    ///   (rollback).
    /// * Ok without fatal errors → success. If we were previously
    ///   `Failing`, clear backoff and move back to `Desired` (unless the
    ///   reconciliation itself promoted us to `Up`).
    /// * Err → transient failure. Transition to `Failing` if not already,
    ///   and bump the in-memory backoff so the next attempt is delayed.
    async fn run_reconcile_active(&mut self, deployment: &Deployment) -> anyhow::Result<()> {
        let sid = short_id(deployment.deployment.as_str()).to_string();
        let was_failing = deployment.state == DeploymentState::Failing;

        match self.reconcile_active(deployment).await {
            Ok(had_fatal_errors) => {
                if had_fatal_errors {
                    tracing::error!("{sid} fatal compile errors; marking FAILED");
                    self.log_publisher
                        .error(format!("{sid} failed fatally: compile errors"))
                        .await;
                    self.client.set(DeploymentState::Failed).await?;
                    self.backoff = None;
                    self.attempt_rollback(&sid).await?;
                    return Ok(());
                }

                if was_failing {
                    self.backoff = None;
                    // The reconciliation may have already promoted us to
                    // `Up`; only move back to `Desired` if we're still
                    // `Failing`.
                    let current = self.client.get().await?;
                    if current.state == DeploymentState::Failing {
                        self.client.set(DeploymentState::Desired).await?;
                        self.log_publisher
                            .info(format!("{sid} recovered; resuming as DESIRED"))
                            .await;
                    }
                }
                Ok(())
            }
            Err(error) => {
                tracing::warn!("{sid} transient error: {error:#}");
                self.log_publisher
                    .warn(format!("{sid} transient error: {error}"))
                    .await;
                if !was_failing {
                    self.client.set(DeploymentState::Failing).await?;
                }
                self.backoff = Some(BackoffState::bump(self.backoff, Instant::now()));
                Ok(())
            }
        }
    }

    /// Attempt to roll back to a predecessor deployment after a fatal
    /// failure. Best-effort: a failure to find or promote a target is
    /// logged but not propagated, since the failing deployment has
    /// already been marked `Failed`.
    async fn attempt_rollback(&self, sid: &str) -> anyhow::Result<()> {
        match self.client.rollback_target().await? {
            Some(target) => {
                let target_sid = short_id(target.deployment.as_str()).to_string();
                let target_client = self
                    .cdb_client
                    .repo(target.repo.clone())
                    .deployment(target.environment.clone(), target.deployment.clone());
                target_client.make_desired().await?;
                tracing::info!("{sid} rolled back to {target_sid}");
                self.log_publisher
                    .info(format!("rolled back {sid} to {target_sid}"))
                    .await;
            }
            None => {
                tracing::warn!("{sid} no rollback target found");
                self.log_publisher
                    .error(format!("no rollback target for failed {sid}"))
                    .await;
            }
        }
        Ok(())
    }

    /// Collect all resources in the environment namespace into a `Vec`.
    async fn collect_all_resources(&self) -> anyhow::Result<Vec<rdb::Resource>> {
        let mut all_resources = Vec::new();
        let mut resources = self.namespace.list_resources().await?;
        while let Some(resource) = resources.try_next().await? {
            all_resources.push(resource);
        }
        Ok(all_resources)
    }

    /// Perform one iteration of reconciliation for an active deployment
    /// (`Desired` or `Failing`). Returns `true` if compilation produced
    /// fatal errors — the caller is responsible for escalating the state.
    ///
    /// Any error returned is considered *transient*: the caller will bump
    /// the backoff and retry later.
    async fn reconcile_active(&mut self, deployment: &Deployment) -> anyhow::Result<bool> {
        let deployment_id = deployment.deployment.clone();
        let sid = short_id(deployment_id.as_str()).to_string();
        tracing::info!("{sid} reconciling");

        // If the commit has no Main.scl, there is nothing to evaluate.
        // Log an info note and transition directly to Up instead of
        // treating it as a compilation error.
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
            return Ok(false);
        }

        let outcome = self.compile_and_evaluate().await?;

        // Fatal compile errors short-circuit: skip teardown work and
        // return early so the caller can escalate. We intentionally do
        // NOT transition volatile or partial-evaluation bookkeeping on a
        // failed compile, because the evaluation tree is unreliable.
        if outcome.had_fatal_errors {
            return Ok(true);
        }

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

                if has_volatile {
                    // Has at least one intrinsically-volatile resource —
                    // stay in Desired so reconciliation keeps probing.
                } else if outcome.has_volatile_cross_repo_pins {
                    tracing::info!("{sid} has volatile cross-repo pins; staying in Desired");
                } else {
                    tracing::info!("{sid} all resources non-volatile; transitioning to UP");
                    self.client.set(DeploymentState::Up).await?;
                    self.log_publisher
                        .info(format!("{sid} is up (all resources non-volatile)"))
                        .await;
                }
            }
            EvalCompleteness::Partial => {
                tracing::info!("evaluation incomplete; deferring superseded deployment teardown");
            }
        }

        Ok(false)
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
        let all_resources = self.collect_all_resources().await?;

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

        let params = DestroyParams {
            all_resources: &all_resources,
            environment_qid: &self.environment_qid,
            deployment_id,
            rtq_publisher: &self.rtq_publisher,
            log_publisher: &self.log_publisher,
        };

        // Don't let untouched owned resources block each other.
        destroy_resources(&untouched_owned, &params, |resource| {
            resource.owner.as_deref() == Some(owner_deployment_qid)
                && !touched_resource_ids.contains(&resource_id_from(resource))
        })
        .await?;

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

        // Resolve cross-repo dependencies, if any. Manifest parsing itself
        // only needs the local package + the standard library.
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
                completeness: EvalCompleteness::Partial,
                touched_resource_ids: HashSet::new(),
                fully_explored: false,
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

        // Collect the set of superseded deployment QIDs so we can validate
        // that adoption only happens from deployments we legitimately supersede.
        let mut superseded_deployment_qids = HashSet::new();
        for superseded in self.client.superseded().await? {
            let dep = superseded.get().await?;
            superseded_deployment_qids.insert(dep.deployment_qid().to_string());
        }

        let (effects_tx, effects_rx) = mpsc::unbounded_channel();
        let environment_qid_str = self.environment_qid.to_string();
        let local_deployment_qid = self.client.deployment_qid();
        let mut eval_ctx = sclc::EvalCtx::new(
            effects_tx,
            environment_qid_str,
            local_deployment_qid.clone(),
        );

        // Register foreign-package owners so the evaluator stamps the right
        // owner on effects produced by foreign global expressions, and
        // pre-load each foreign deployment's resources so remote-state
        // reads can return concrete outputs.
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

        let has_volatile_cross_repo_pins = cross_repo_finder
            .as_ref()
            .is_some_and(|f| f.has_volatile_pins());
        let effects_task = task::spawn(
            drain_effects(
                effects_rx,
                local_deployment_qid,
                deployment_id,
                self.environment_qid.clone(),
                self.rtq_publisher.clone(),
                self.log_publisher.clone(),
                unowned_resource_owner_by_id,
                superseded_deployment_qids,
                volatile_resource_ids,
                has_volatile_cross_repo_pins,
            )
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
        // For manifest parsing only, a (user + std) finder is sufficient —
        // `Package.scle` is expected to import only `Std/...`.
        let manifest_finder = sclc::build_default_finder(Arc::clone(&user_pkg));
        let manifest = match sclc::load_manifest(Arc::clone(&user_pkg), manifest_finder).await {
            Ok(Some(m)) => m,
            Ok(None) => return Ok(None),
            Err(e) => {
                // Surface the error as a deployment failure so the operator
                // can see why the deployment didn't proceed.
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
/// local environment → standard library. The CDB fallback preserves the
/// pre-cross-repo behaviour: a deployment can still resolve `Org/Repo`
/// imports against active deployments in its own environment without
/// declaring them in the manifest. (The cross-repo finder takes
/// precedence when the manifest declares a specifier.)
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

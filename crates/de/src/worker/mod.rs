pub(crate) mod eval;

use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
    time::Duration,
};

use cdb::{Deployment, DeploymentClient, DeploymentState};
use futures_util::TryStreamExt;
use tokio::{
    sync::oneshot::{self, error::TryRecvError},
    time::{Instant, sleep_until},
};

use crate::backoff::backoff_duration;
use crate::reporter::{
    FailureKind, IterationOutcome, build_deployment_report, probe_deployment,
    probe_resource_open_crash, publish_report,
};
use crate::util::{resource_id_from, resource_ref, short_id};

pub(crate) struct CachedCompile {
    pub(crate) key: BTreeMap<ids::RepoQid, ids::DeploymentQid>,
    pub(crate) asg: Arc<sclc::Asg>,
}

pub(crate) struct Worker {
    pub(crate) client: DeploymentClient,
    pub(crate) cdb_client: cdb::Client,
    pub(crate) rdb_client: rdb::Client,
    pub(crate) environment_qid: ids::EnvironmentQid,
    pub(crate) namespace: rdb::NamespaceClient,
    pub(crate) rtq_publisher: rtq::Publisher,
    pub(crate) rq_publisher: rq::Publisher,
    pub(crate) sdb_client: sdb::Client,
    pub(crate) log_publisher: ldb::NamespacePublisher,
    /// Tracks when the last failure occurred, used together with the
    /// persisted `failures` counter to compute exponential backoff.
    pub(crate) last_failure_at: Option<Instant>,
    /// Cached compiled ASG from the previous iteration, keyed by the
    /// resolved cross-repo dependency map. Reused when the resolved map
    /// is unchanged; cleared on compile error or when the key differs.
    pub(crate) cached_compile: Option<CachedCompile>,
    /// Latched once the deployment's terminal status report has been emitted
    /// onto the RQ. The terminal report is, by design, the last report ever
    /// emitted for this deployment.
    pub(crate) terminal_reported: bool,
}

/// What a single worker iteration observed and decided.
///
/// Returned by [`Worker::work`] so [`Worker::run_loop`] can construct the
/// per-iteration RQ report uniformly across the four state branches.
struct WorkResult {
    /// Whether the worker loop should keep iterating after this turn.
    keep_running: bool,
    /// The deployment state observed at the start of this iteration. Used to
    /// fill the report's deployment-scoped operational state.
    state: DeploymentState,
    /// Producer-classified outcome of the iteration. Always present — even
    /// idle DOWN/LINGERING iterations and "check"-only iterations report
    /// success; this gives the RE a uniform liveness signal.
    outcome: IterationOutcome,
}

impl Worker {
    pub(crate) async fn run_loop(mut self, mut rx: oneshot::Receiver<()>) {
        loop {
            let next_loop = Instant::now() + Duration::from_secs(5);

            match rx.try_recv() {
                Ok(()) | Err(TryRecvError::Closed) => return,
                Err(TryRecvError::Empty) => {
                    let stop = self.work_and_report().await;
                    if stop {
                        return;
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

    /// Run a single iteration of the worker, then unconditionally publish a
    /// status report and run the SDB preview reads. Returns `true` when the
    /// outer loop should stop.
    ///
    /// This wrapper enforces the *heartbeat* property: every iteration emits
    /// exactly one report, regardless of state or outcome, and reporting must
    /// not be influenced by the SDB reads (the producer is one-way).
    async fn work_and_report(&mut self) -> bool {
        // The terminal report has already been emitted; no further reports
        // for this deployment, ever. Stop the loop too — there's nothing left
        // for the worker to do.
        if self.terminal_reported {
            return true;
        }

        let started_at = Instant::now();
        let result = self.work().await;
        let elapsed = started_at.elapsed();

        let work_result = match result {
            Ok(wr) => wr,
            Err(error) => {
                // Any propagated `?` error (CDB / RDB / RTQ / LDB / similar
                // infrastructure failures) is classified as `SystemError`.
                // The state may be unknown if the very first read failed; in
                // that case fall back to `Desired` for reporting purposes —
                // we still want a heartbeat to go out so the RE can detect
                // a stuck worker.
                tracing::error!("{error}");
                let outcome = IterationOutcome::Failure {
                    kind: FailureKind::SystemError,
                    error_message: format!("{error:#}"),
                };
                WorkResult {
                    keep_running: true,
                    state: DeploymentState::Desired,
                    outcome,
                }
            }
        };

        let deployment_qid = self.client.deployment_qid();
        let is_terminal = matches!(work_result.state, DeploymentState::Down);

        // Build & publish the report. Reporting failures are logged but never
        // propagate — the DE keeps reconciling regardless of broker health.
        let report = build_deployment_report(
            deployment_qid.clone(),
            work_result.state,
            is_terminal,
            elapsed,
            work_result.outcome,
        );
        publish_report(&self.rq_publisher, &report).await;

        // Read SDB signals (preview only; the legacy `cdb.failures` path
        // remains authoritative for backoff and eligibility decisions until
        // the `backoff_migration` task lands). These reads must not be
        // observable to the RE, so they happen *after* the report is on the
        // wire.
        self.preview_sdb_signals(&deployment_qid).await;

        if is_terminal {
            self.terminal_reported = true;
            tracing::info!(
                deployment = %deployment_qid,
                "emitted terminal status report; stopping worker",
            );
            // The terminal report is the last report ever emitted; stop the
            // loop unconditionally.
            return true;
        }

        // Otherwise the inner work() decision dictates whether we keep going.
        !work_result.keep_running
    }

    /// Read SDB signals for backoff and eligibility decisions, currently used
    /// only for observability. Errors are logged at debug level so transient
    /// SDB outages do not spam operator logs.
    async fn preview_sdb_signals(&self, deployment_qid: &ids::DeploymentQid) {
        let mut preview = probe_deployment(&self.sdb_client, deployment_qid).await;

        // Resource-level open-Crash check across the deployment's owned
        // resources. We reuse the existing namespace iterator the rest of the
        // worker walks, which is RDB-cheap.
        let owner_qid_str = deployment_qid.to_string();
        match self.namespace.list_resources().await {
            Ok(mut resources) => loop {
                match resources.try_next().await {
                    Ok(Some(resource)) => {
                        if resource.owner.as_deref() != Some(owner_qid_str.as_str()) {
                            continue;
                        }
                        let resource_id = resource_id_from(&resource);
                        let resource_qid = self.environment_qid.resource(resource_id).to_string();
                        if probe_resource_open_crash(&self.sdb_client, &resource_qid).await {
                            preview.any_resource_has_open_crash = true;
                            // We only need the existence; further iteration is
                            // wasted work for the preview path.
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        tracing::debug!(
                            error = %error,
                            "failed to enumerate resources for SDB preview; ignoring",
                        );
                        break;
                    }
                }
            },
            Err(error) => {
                tracing::debug!(
                    error = %error,
                    "failed to list resources for SDB preview; ignoring",
                );
            }
        }

        tracing::debug!(
            deployment = %deployment_qid,
            sdb_consecutive_failures = ?preview.consecutive_failure_count,
            sdb_deployment_open_crash = preview.deployment_has_open_crash,
            sdb_any_resource_open_crash = preview.any_resource_has_open_crash,
            sdb_any_open_crash = preview.any_open_crash(),
            "SDB backoff/eligibility preview (observational only)",
        );
    }

    /// Run the per-state handler for the current deployment, returning the
    /// observed state and the producer's classified iteration outcome.
    async fn work(&mut self) -> anyhow::Result<WorkResult> {
        let deployment = self.client.get().await?;
        let sid = short_id(deployment.deployment.as_str()).to_string();
        let state = deployment.state;

        match state {
            DeploymentState::Down => {
                tracing::info!("{sid} down, waiting to be decommissioned...");
                Ok(WorkResult {
                    keep_running: true,
                    state,
                    outcome: IterationOutcome::Success,
                })
            }

            DeploymentState::Desired => self.run_desired(&deployment).await,

            DeploymentState::Lingering => {
                self.run_lingering(&deployment).await?;
                Ok(WorkResult {
                    keep_running: true,
                    state,
                    outcome: IterationOutcome::Success,
                })
            }

            DeploymentState::Undesired => {
                self.run_undesired(&deployment).await?;
                Ok(WorkResult {
                    keep_running: true,
                    state,
                    outcome: IterationOutcome::Success,
                })
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
    /// Returns a [`WorkResult`] whose `keep_running` is `false` to signal that
    /// the processing loop should stop (e.g., when there is no `Main.scl` and
    /// thus no volatile resources).
    async fn run_desired(&mut self, deployment: &Deployment) -> anyhow::Result<WorkResult> {
        let sid = short_id(deployment.deployment.as_str()).to_string();
        let state = DeploymentState::Desired;

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
                    // Backed-off iterations still emit a heartbeat; the
                    // producer reports success because the DE-internal
                    // backoff is operating as designed.
                    return Ok(WorkResult {
                        keep_running: true,
                        state,
                        outcome: IterationOutcome::Success,
                    });
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
            return Ok(WorkResult {
                keep_running: true,
                state,
                outcome: IterationOutcome::Success,
            });
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
            if !deployment.bootstrapped {
                tracing::info!("{sid} no Main.scl; marking bootstrapped");
                self.client.set_progress(true, 0).await?;
                self.transition_superseded_to_undesired().await?;
            }
            return Ok(WorkResult {
                keep_running: false,
                state,
                outcome: IterationOutcome::Success,
            });
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
                    return Ok(WorkResult {
                        keep_running: true,
                        state,
                        outcome: IterationOutcome::Failure {
                            kind: FailureKind::BadConfiguration,
                            error_message: format!(
                                "{sid} compile errors (failures={new_failures})"
                            ),
                        },
                    });
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

                Ok(WorkResult {
                    keep_running: true,
                    state,
                    outcome: IterationOutcome::Success,
                })
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
                Ok(WorkResult {
                    keep_running: true,
                    state,
                    outcome: IterationOutcome::Failure {
                        kind: FailureKind::SystemError,
                        error_message: format!("{sid} transient error: {error:#}"),
                    },
                })
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

    pub(crate) async fn publish_diagnostics(&self, diags: &sclc::DiagList) {
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
                    crate::util::diag_severity(diag.level()),
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

    /// Load the local repo's `Package.scle` (if any) and build a
    /// [`sclc::CrossRepoPackageFinder`] from its declared dependencies.
    /// Returns `Ok(None)` when the manifest is absent or has no deps.
    pub(crate) async fn build_cross_repo_finder(
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

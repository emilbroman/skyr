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
    pub(crate) log_publisher: ldb::NamespacePublisher,
    /// Tracks when the last failure occurred, used together with the
    /// persisted `failures` counter to compute exponential backoff.
    pub(crate) last_failure_at: Option<Instant>,
    /// Cached compiled ASG from the previous iteration, keyed by the
    /// resolved cross-repo dependency map. Reused when the resolved map
    /// is unchanged; cleared on compile error or when the key differs.
    pub(crate) cached_compile: Option<CachedCompile>,
}

impl Worker {
    pub(crate) async fn run_loop(mut self, mut rx: oneshot::Receiver<()>) {
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

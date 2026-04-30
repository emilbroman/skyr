use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
};

use futures_util::TryStreamExt;
use tokio::task;
use tracing::Instrument;

use super::{CachedCompile, Worker};
use crate::finder::build_full_finder;
use crate::util::{
    enqueue_message, extract_deployment_identity, map_dependencies, resource_id_from, resource_ref,
    serialize_inputs,
};

pub(crate) struct EvalOutcome {
    /// Resource IDs referenced during evaluation. When `fully_explored` is
    /// true, this is the complete set — any owned resource NOT in this set
    /// is no longer desired and should be destroyed.
    pub(crate) touched_resource_ids: HashSet<ids::ResourceId>,
    /// True when no Create/Update effects were emitted, meaning every
    /// resource function returned concrete outputs and all code paths
    /// were fully evaluated.
    pub(crate) fully_explored: bool,
    /// True when at least one effect was emitted (Create, Update, Adopt, etc).
    pub(crate) had_effect: bool,
    /// `Some(message)` when compilation produced one or more `Error`-severity
    /// diagnostics. The message is the full DiagList rendered as one
    /// `LEVEL: <module>:<span>: <diag>` line per diagnostic, joined by `\n`,
    /// suitable for surfacing to end users via the RQ failure report.
    pub(crate) compile_error_message: Option<String>,
}

impl Worker {
    pub(crate) async fn compile_and_evaluate(&mut self) -> anyhow::Result<EvalOutcome> {
        let user_pkg: Arc<dyn sclc::Package> = Arc::new(self.client.commit_client().clone());
        let repo_qid = self.client.repo_qid().clone();

        let cross_repo_finder = self
            .build_cross_repo_finder(Arc::clone(&user_pkg), &repo_qid)
            .await?;

        // Eagerly resolve every declared cross-repo dep so we can use the
        // resolved commits as a cache key. This also populates the finder's
        // internal owner table, which `resolved_owners()` reads below.
        let resolved_key: BTreeMap<ids::RepoQid, ids::CommitHash> = match &cross_repo_finder {
            Some(f) => f.resolve_all().await?,
            None => BTreeMap::new(),
        };

        let cached_asg = self
            .cached_compile
            .as_ref()
            .filter(|c| c.key == resolved_key)
            .map(|c| Arc::clone(&c.asg));
        let cache_hit = cached_asg.is_some();
        let asg: Arc<sclc::Asg> = if let Some(asg) = cached_asg {
            tracing::debug!("compile cache hit");
            asg
        } else {
            tracing::debug!("compile cache miss; recompiling");
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
                self.cached_compile = None;
                tracing::info!("compile produced errors; skipping evaluation");
                let compile_error_message = diagnosed
                    .diags()
                    .iter()
                    .map(|diag| {
                        let (module_id, span) = diag.locate();
                        let level = match diag.level() {
                            sclc::DiagLevel::Error => "ERROR",
                            sclc::DiagLevel::Warning => "WARNING",
                        };
                        format!("{level}: {module_id}:{span}: {diag}")
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                return Ok(EvalOutcome {
                    touched_resource_ids: HashSet::new(),
                    fully_explored: false,
                    had_effect: false,
                    compile_error_message: Some(compile_error_message),
                });
            }

            let asg = Arc::new(diagnosed.into_inner());
            self.cached_compile = Some(CachedCompile {
                key: resolved_key,
                asg: Arc::clone(&asg),
            });
            asg
        };
        let full_deployment_qid = self.client.deployment_qid();
        let owner_deployment_qid = full_deployment_qid.to_string();
        let deployment_id = full_deployment_qid.deployment.clone();

        let (effects_tx, mut effects_rx) = tokio::sync::mpsc::unbounded_channel();
        let environment_qid_str = self.environment_qid.to_string();
        let local_deployment_qid = self.client.deployment_qid();
        let mut eval_ctx = sclc::EvalCtx::new(
            effects_tx,
            environment_qid_str,
            local_deployment_qid.clone(),
        );

        if let Some(finder) = &cross_repo_finder {
            for (foreign_repo, foreign_owner) in finder.resolved_owners().await {
                if !cache_hit {
                    self.log_publisher
                        .info(format!("Resolved package {foreign_owner}"))
                        .await;
                }
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

            // Hash-pinned dependencies have no deployment owner. Mark them as
            // orphan so the evaluator stamps their effects with `None`, and
            // the drain task below logs them as errors.
            for orphan_repo in finder.resolved_orphans().await {
                if !cache_hit {
                    self.log_publisher
                        .info(format!(
                            "Resolved package {orphan_repo} (hash-pinned, orphan)"
                        ))
                        .await;
                }
                let pkg_id = sclc::package_id_for_repo(&orphan_repo);
                eval_ctx.set_package_orphan(pkg_id);
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
        let effects_task = task::spawn(
            {
                async move {
                    let mut had_effect = false;
                    let mut had_mutation = false;
                    let mut touched_resource_ids = HashSet::new();
                    while let Some(effect) = effects_rx.recv().await {
                        match effect.owner() {
                            None => {
                                let id = effect.id();
                                tracing::warn!(
                                    resource = %id,
                                    "orphan effect emitted from a hash-pinned package",
                                );
                                log_publisher
                                    .error(format!(
                                        "Resource {id} was emitted from a hash-pinned \
                                         dependency with no deployment owner; dropping. \
                                         Pin the dependency to a branch or tag to attribute \
                                         its resources to the foreign deployment, or move the \
                                         resource into the local package.",
                                    ))
                                    .await;
                                continue;
                            }
                            Some(owner) if owner != &local_deployment_qid_for_drain => {
                                tracing::debug!(
                                    owner = %owner,
                                    local_owner = %local_deployment_qid_for_drain,
                                    "dropping foreign-owned effect",
                                );
                                continue;
                            }
                            Some(_) => {}
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
                                    let from_deployment_id =
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
                                    let from_deployment_id = match extract_deployment_identity(
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
                        touched_resource_ids,
                        fully_explored: !had_mutation,
                        had_effect,
                        compile_error_message: None,
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

use std::pin::Pin;
use std::time::Duration;

use chrono::{TimeZone, Utc};
use futures_util::{Stream, StreamExt, TryStreamExt};
use juniper::FieldResult;

use crate::schema::{Log, Severity};
use crate::{Context, field_error, internal_error};

pub(crate) type LogStream = Pin<Box<dyn Stream<Item = Log> + Send>>;

pub(crate) struct Subscription;

#[juniper::graphql_subscription(Context = Context)]
impl Subscription {
    async fn deployment_logs(
        context: &Context,
        deployment_id: String,
        initial_amount: Option<i32>,
    ) -> FieldResult<LogStream> {
        let (_, user) = context.check_auth().await?;

        let deployment_qid: ids::DeploymentQid = deployment_id
            .parse()
            .map_err(|_| field_error("invalid deployment id"))?;
        let _ = context
            .home_region_for_repo(deployment_qid.repo_qid())
            .await?;
        let org_id = deployment_qid.repo_qid().org.clone();
        let organization = org_id.to_string();
        let org_region = context.home_region_for_org(&org_id).await?;

        if organization != user.username {
            let is_member = context
                .udb_for_region(&org_region)
                .await?
                .org(&organization)
                .members()
                .contains(&user.username)
                .await
                .map_err(|e| {
                    tracing::error!("Failed to check org membership: {}", e);
                    internal_error()
                })?;
            if !is_member {
                tracing::warn!(
                    "Rejected deployment logs subscription: user is not a member of organization: deployment={} user={}",
                    deployment_id,
                    user.username
                );
                return Err(field_error("Permission denied"));
            }
        }

        let initial_amount = initial_amount.unwrap_or(1000).max(0) as u64;

        let namespace = context
            .ldb_consumer
            .namespace(deployment_id)
            .await
            .map_err(|e| {
                tracing::error!("Failed to prepare deployment logs subscription consumer: {e}");
                field_error("failed to tail logs")
            })?;
        let mut inner = namespace
            .tail(ldb::TailConfig {
                follow: true,
                start_from: ldb::StartFrom::End(initial_amount),
            })
            .await
            .map_err(|e| {
                tracing::error!("Failed to tail deployment logs subscription: {e}");
                field_error("failed to tail logs")
            })?;

        Ok(Box::pin(async_stream::stream! {
            while let Some(item) = inner.next().await {
                match item {
                    Ok((timestamp, severity, message)) => {
                        yield Log {
                            severity: severity.into(),
                            timestamp: format_timestamp(timestamp),
                            message,
                        };
                    }
                    Err(error) => {
                        tracing::warn!("Error while streaming deployment logs: {}", error);
                        break;
                    }
                }
            }
        }))
    }

    async fn environment_logs(
        context: &Context,
        environment_qid: String,
        initial_amount: Option<i32>,
    ) -> FieldResult<LogStream> {
        let (_, user) = context.check_auth().await?;

        let env_qid: ids::EnvironmentQid = environment_qid
            .parse()
            .map_err(|_| field_error("invalid environment QID"))?;

        let repo_region = context.home_region_for_repo(&env_qid.repo).await?;
        let org_region = context.home_region_for_org(&env_qid.repo.org).await?;

        let organization = env_qid.repo.org.to_string();
        if organization != user.username {
            let is_member = context
                .udb_for_region(&org_region)
                .await?
                .org(&organization)
                .members()
                .contains(&user.username)
                .await
                .map_err(|e| {
                    tracing::error!("Failed to check org membership: {}", e);
                    internal_error()
                })?;
            if !is_member {
                tracing::warn!(
                    "Rejected environment logs subscription: user is not a member of organization: environment={} user={}",
                    environment_qid,
                    user.username
                );
                return Err(field_error("Permission denied"));
            }
        }

        let initial_amount = initial_amount.unwrap_or(1000).max(0) as u64;

        let consumer = context.ldb_consumer.clone();
        let cdb_client = context.cdb_for_region(&repo_region).await?;

        Ok(Box::pin(async_stream::stream! {
            let mut merged = futures_util::stream::SelectAll::new();
            let mut subscribed: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
            let mut poll_interval = tokio::time::interval(Duration::from_secs(3));

            loop {
                tokio::select! {
                    biased;

                    Some(item) = merged.next(), if !merged.is_empty() => {
                        match item {
                            Ok((timestamp, severity, message)) => {
                                let severity = Severity::from(severity);
                                yield Log {
                                    severity,
                                    timestamp: format_timestamp(timestamp),
                                    message,
                                };
                            }
                            Err(error) => {
                                tracing::warn!("Error while streaming environment logs: {}", error);
                                break;
                            }
                        }
                    }

                    _ = poll_interval.tick() => {
                        let deployments = match cdb_client
                            .repo(env_qid.repo.clone())
                            .deployments()
                            .await
                        {
                            Ok(stream) => match stream.try_collect::<Vec<_>>().await {
                                Ok(deployments) => deployments,
                                Err(e) => {
                                    tracing::warn!("Failed to read deployments while polling for environment logs: {e}");
                                    continue;
                                }
                            },
                            Err(e) => {
                                tracing::warn!("Failed to list deployments while polling for environment logs: {e}");
                                continue;
                            }
                        };

                        for deployment in deployments {
                            if deployment.environment_qid() != env_qid {
                                continue;
                            }
                            let deployment_qid = deployment.deployment_qid().to_string();
                            if !subscribed.insert(deployment_qid.clone()) {
                                continue;
                            }

                            let namespace = match consumer.namespace(deployment_qid.clone()).await {
                                Ok(ns) => ns,
                                Err(e) => {
                                    tracing::warn!("Failed to prepare log consumer for deployment {deployment_qid}: {e}");
                                    subscribed.remove(&deployment_qid);
                                    continue;
                                }
                            };
                            match namespace
                                .tail(ldb::TailConfig {
                                    follow: true,
                                    start_from: ldb::StartFrom::End(initial_amount),
                                })
                                .await
                            {
                                Ok(stream) => {
                                    tracing::info!("Started log consumer for deployment {deployment_qid}");
                                    merged.push(stream);
                                }
                                Err(e) => {
                                    tracing::warn!("Failed to tail logs for deployment {deployment_qid}: {e}");
                                    subscribed.remove(&deployment_qid);
                                }
                            }
                        }
                    }
                }
            }
        }))
    }

    async fn resource_logs(
        context: &Context,
        resource_qid: String,
        initial_amount: Option<i32>,
    ) -> FieldResult<LogStream> {
        let (_, user) = context.check_auth().await?;

        let parsed_qid: ids::ResourceQid = resource_qid
            .parse()
            .map_err(|_| field_error("invalid resource QID"))?;
        let _ = context
            .home_region_for_repo(&parsed_qid.environment_qid().repo)
            .await?;
        let org_id = parsed_qid.environment_qid().repo.org.clone();
        let organization = org_id.to_string();
        let org_region = context.home_region_for_org(&org_id).await?;

        if organization != user.username {
            let is_member = context
                .udb_for_region(&org_region)
                .await?
                .org(&organization)
                .members()
                .contains(&user.username)
                .await
                .map_err(|e| {
                    tracing::error!("Failed to check org membership: {}", e);
                    internal_error()
                })?;
            if !is_member {
                tracing::warn!(
                    "Rejected resource logs subscription: user is not a member of organization: resource={} user={}",
                    resource_qid,
                    user.username
                );
                return Err(field_error("Permission denied"));
            }
        }

        let initial_amount = initial_amount.unwrap_or(1000).max(0) as u64;

        let namespace = context
            .ldb_consumer
            .namespace(resource_qid.clone())
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to prepare resource logs subscription consumer for {resource_qid}: {e}"
                );
                field_error("failed to tail logs")
            })?;
        let mut inner = namespace
            .tail(ldb::TailConfig {
                follow: true,
                start_from: ldb::StartFrom::End(initial_amount),
            })
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to tail resource logs subscription for {resource_qid}: {e}"
                );
                field_error("failed to tail logs")
            })?;

        Ok(Box::pin(async_stream::stream! {
            while let Some(item) = inner.next().await {
                match item {
                    Ok((timestamp, severity, message)) => {
                        yield Log {
                            severity: severity.into(),
                            timestamp: format_timestamp(timestamp),
                            message,
                        };
                    }
                    Err(error) => {
                        tracing::warn!("Error while streaming resource logs: {}", error);
                        break;
                    }
                }
            }
        }))
    }
}

pub(crate) async fn load_logs(
    context: &Context,
    namespace: String,
    amount: u64,
) -> anyhow::Result<Vec<Log>> {
    let namespace = context.ldb_consumer.namespace(namespace).await?;
    let mut stream = namespace
        .tail(ldb::TailConfig {
            follow: false,
            start_from: ldb::StartFrom::End(amount),
        })
        .await?;

    let mut logs = Vec::new();
    while let Some(item) = stream.next().await {
        let (timestamp, severity, message) = item?;
        logs.push(Log {
            severity: severity.into(),
            timestamp: format_timestamp(timestamp),
            message,
        });
    }

    Ok(logs)
}

fn format_timestamp(timestamp_millis: u64) -> String {
    let Ok(millis) = i64::try_from(timestamp_millis) else {
        tracing::warn!("Timestamp overflow: {timestamp_millis} exceeds i64 range");
        return format!("<invalid timestamp: {timestamp_millis}>");
    };
    match Utc.timestamp_millis_opt(millis) {
        chrono::LocalResult::Single(timestamp) => timestamp.to_rfc3339(),
        _ => {
            tracing::warn!("Timestamp out of representable range: {millis}ms");
            format!("<invalid timestamp: {millis}ms>")
        }
    }
}

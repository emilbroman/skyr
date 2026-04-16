use anyhow::{Context, anyhow};
use clap::{Args, Subcommand};
use graphql_client::GraphQLQuery;
use serde::Serialize;
use serde_json::json;
use std::{collections::BTreeSet, time::Duration};

use crate::{auth, output::OutputFormat, repo, ws};

/// Custom scalar required by `graphql_client` derive for the `JSON` scalar in the schema.
#[allow(clippy::upper_case_acronyms)]
type JSON = serde_json::Value;

#[derive(Args, Debug)]
pub struct DeploymentsArgs {
    #[command(subcommand)]
    command: DeploymentsCommand,
    #[arg(long, default_value = "https://skyr.cloud")]
    api_url: String,
}

#[derive(Subcommand, Debug)]
enum DeploymentsCommand {
    List {
        repository: String,
    },
    /// Show or stream deployment logs
    Logs {
        repository: String,
        /// Stream logs continuously
        #[arg(short, long)]
        follow: bool,
        /// Number of log entries to show (or initial entries when following)
        #[arg(short = 'n', long, default_value = "200")]
        latest: i64,
    },
}

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "../api/schema.graphql",
    query_path = "src/graphql/list_repository_deployments.graphql",
    response_derives = "Debug"
)]
struct ListRepositoryDeployments;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "../api/schema.graphql",
    query_path = "src/graphql/deployment_last_logs.graphql",
    response_derives = "Debug"
)]
struct DeploymentLastLogs;

pub async fn run_deployments(args: DeploymentsArgs, format: OutputFormat) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let token = auth::acquire_token(&client, &args.api_url).await?;
    let endpoint = auth::graphql_endpoint(&args.api_url);

    match args.command {
        DeploymentsCommand::List { repository } => {
            list_deployments(&client, &endpoint, &token, &repository, format).await?;
        }
        DeploymentsCommand::Logs {
            repository,
            follow,
            latest,
        } => {
            if follow {
                stream_deployment_logs(&client, &endpoint, &token, &repository, latest).await?;
            } else {
                print_deployment_last_logs(&client, &endpoint, &token, &repository, latest, format)
                    .await?;
            }
        }
    }

    Ok(())
}

#[derive(Serialize)]
struct DeploymentOutput {
    r#ref: String,
    commit: String,
    created_at: String,
    state: String,
    resources: Vec<ResourceOutput>,
}

#[derive(Serialize)]
struct ResourceOutput {
    r#type: String,
    name: String,
    inputs: Option<serde_json::Value>,
    outputs: Option<serde_json::Value>,
    dependencies: Vec<ResourceDependencyOutput>,
}

#[derive(Serialize)]
struct ResourceDependencyOutput {
    r#type: String,
    name: String,
}

async fn list_deployments(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
    repository: &str,
    format: OutputFormat,
) -> anyhow::Result<()> {
    let (_, repository_name) = repo::parse_repository_path(repository)?;

    let environments =
        query_repository_environments(client, endpoint, token, repository_name).await?;

    let deployments: Vec<DeploymentOutput> = environments
        .into_iter()
        .flat_map(|env| {
            env.deployments
                .into_iter()
                .map(|deployment| DeploymentOutput {
                    r#ref: deployment.ref_,
                    commit: deployment.commit.hash,
                    created_at: deployment.created_at,
                    state: format!("{:?}", deployment.state),
                    resources: deployment
                        .resources
                        .into_iter()
                        .map(|resource| ResourceOutput {
                            r#type: resource.type_,
                            name: resource.name,
                            inputs: resource.inputs,
                            outputs: resource.outputs,
                            dependencies: resource
                                .dependencies
                                .into_iter()
                                .map(|dependency| ResourceDependencyOutput {
                                    r#type: dependency.type_,
                                    name: dependency.name,
                                })
                                .collect::<Vec<_>>(),
                        })
                        .collect::<Vec<_>>(),
                })
        })
        .collect();

    match format {
        OutputFormat::Json => crate::output::print_json(&deployments)?,
        OutputFormat::Text => {
            let mut table = crate::output::table("{:<}  {:<}  {:<}  {:<}  {:>}");
            table.add_row(crate::output::row(vec![
                String::from("REF"),
                String::from("COMMIT"),
                String::from("CREATED"),
                String::from("STATE"),
                String::from("RESOURCES"),
            ]));
            for deployment in deployments {
                table.add_row(crate::output::row(vec![
                    deployment.r#ref,
                    crate::output::shorten_commit_hash(&deployment.commit),
                    crate::output::format_created_at(&deployment.created_at),
                    deployment.state,
                    deployment.resources.len().to_string(),
                ]));
            }
            print!("{table}");
        }
    }

    Ok(())
}

#[derive(Serialize)]
struct DeploymentLogOutput {
    deployment_id: String,
    logs: Vec<LogOutput>,
}

#[derive(Serialize)]
struct LogOutput {
    severity: String,
    timestamp: String,
    message: String,
}

async fn print_deployment_last_logs(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
    repository: &str,
    amount: i64,
    format: OutputFormat,
) -> anyhow::Result<()> {
    let (_, repository_name) = repo::parse_repository_path(repository)?;

    let body = DeploymentLastLogs::build_query(deployment_last_logs::Variables {
        amount: Some(amount),
    });
    let data: deployment_last_logs::ResponseData =
        auth::send_graphql(client, endpoint, token, body, "deployment logs").await?;
    let repo = data
        .repositories
        .into_iter()
        .find(|r| r.name == repository_name)
        .ok_or_else(|| anyhow!("repository '{repository_name}' not found"))?;

    let deployments: Vec<_> = repo
        .environments
        .into_iter()
        .flat_map(|env| {
            env.deployments
                .into_iter()
                .filter(|d| {
                    !matches!(d.state, deployment_last_logs::DeploymentState::DOWN)
                        && !d.last_logs.is_empty()
                })
                .map(|d| DeploymentLogOutput {
                    deployment_id: d.id,
                    logs: d
                        .last_logs
                        .into_iter()
                        .map(|l| LogOutput {
                            severity: format!("{:?}", l.severity),
                            timestamp: l.timestamp,
                            message: l.message,
                        })
                        .collect(),
                })
        })
        .collect();

    if deployments.is_empty() {
        return Err(anyhow!(
            "repository '{}' has no active deployments with logs",
            repository
        ));
    }

    match format {
        OutputFormat::Json => crate::output::print_json(&deployments)?,
        OutputFormat::Text => {
            for (i, deployment) in deployments.iter().enumerate() {
                if i > 0 {
                    println!();
                }
                println!("==> {} <==", deployment.deployment_id);
                for log in &deployment.logs {
                    println!("[{}] [{}] {}", log.timestamp, log.severity, log.message);
                }
            }
        }
    }

    Ok(())
}

async fn stream_deployment_logs(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
    repository: &str,
    initial_amount: i64,
) -> anyhow::Result<()> {
    let (_, repository_name) = repo::parse_repository_path(repository)?;
    let ws_endpoint = ws::graphql_ws_endpoint(endpoint)?;
    let mut subscribed = BTreeSet::new();
    let mut saw_any = false;
    let (task_events_tx, mut task_events_rx) =
        tokio::sync::mpsc::unbounded_channel::<(String, anyhow::Result<()>)>();

    loop {
        let environments =
            query_repository_environments(client, endpoint, token, repository_name).await?;

        let environment_qids: BTreeSet<String> = environments
            .into_iter()
            .filter(|env| {
                env.deployments
                    .iter()
                    .any(|d| !matches!(d.state, list_repository_deployments::DeploymentState::DOWN))
            })
            .map(|env| env.qid)
            .collect();

        if environment_qids.is_empty() && !saw_any {
            return Err(anyhow!(
                "repository '{}' has no active deployments (all are DOWN)",
                repository
            ));
        }

        for environment_qid in environment_qids {
            if subscribed.insert(environment_qid.clone()) {
                saw_any = true;
                let ws_endpoint = ws_endpoint.clone();
                let token = token.to_owned();
                let task_events_tx = task_events_tx.clone();
                let env_qid = environment_qid.clone();
                tokio::spawn(async move {
                    let result = ws::stream_subscription(
                        &ws_endpoint,
                        &token,
                        ws::SubscriptionParams {
                            subscription_id: &env_qid,
                            query: include_str!("graphql/deployment_logs_subscription.graphql"),
                            operation_name: "EnvironmentLogs",
                            variables: json!({
                                "environmentQid": env_qid,
                                "initialAmount": initial_amount
                            }),
                            log_field_name: "environmentLogs",
                        },
                        |log| {
                            println!(
                                "[{}] [{}] [{}] {}",
                                env_qid, log.timestamp, log.severity, log.message
                            );
                        },
                    )
                    .await;
                    let _ = task_events_tx.send((env_qid, result));
                });
            }
        }

        let sleep = tokio::time::sleep(Duration::from_secs(3));
        tokio::pin!(sleep);

        tokio::select! {
            signal = tokio::signal::ctrl_c() => {
                signal.context("failed to wait for ctrl-c")?;
                return Ok(());
            }
            event = task_events_rx.recv() => {
                let Some((environment_qid, result)) = event else {
                    continue;
                };
                subscribed.remove(&environment_qid);
                result.with_context(|| format!("log stream failed for environment {environment_qid}"))?;
            }
            _ = &mut sleep => {}
        }
    }
}

async fn query_repository_environments(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
    repository_name: &str,
) -> anyhow::Result<
    Vec<list_repository_deployments::ListRepositoryDeploymentsRepositoriesEnvironments>,
> {
    let body = ListRepositoryDeployments::build_query(list_repository_deployments::Variables {});
    let data: list_repository_deployments::ResponseData =
        auth::send_graphql(client, endpoint, token, body, "deployment list").await?;
    let repository = data
        .repositories
        .into_iter()
        .find(|repo| repo.name == repository_name)
        .ok_or_else(|| anyhow!("repository '{repository_name}' not found"))?;

    Ok(repository.environments)
}

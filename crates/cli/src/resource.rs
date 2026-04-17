use anyhow::{Context, anyhow};
use clap::{Args, Subcommand};
use graphql_client::GraphQLQuery;
use serde::Serialize;
use serde_json::json;

use crate::{auth, auth::JSON, output::OutputFormat, repo, ws};

#[derive(Args, Debug)]
pub struct ResourcesArgs {
    #[command(subcommand)]
    command: ResourcesCommand,
    #[arg(long, default_value = "https://skyr.cloud")]
    api_url: String,
}

#[derive(Subcommand, Debug)]
enum ResourcesCommand {
    /// List resources for a repository, optionally filtered by environment
    List {
        repository: String,
        /// Filter resources by environment name
        #[arg(long)]
        environment: Option<String>,
    },
    /// Show or stream logs for one or more resources (interlaced)
    Logs {
        /// Resource QIDs (e.g. org/repo::env::Type:name), at least one required
        #[arg(required = true, num_args = 1..)]
        resource_qids: Vec<String>,
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
    query_path = "src/graphql/list_repository_resources.graphql",
    response_derives = "Debug"
)]
struct ListRepositoryResources;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "../api/schema.graphql",
    query_path = "src/graphql/resource_last_logs.graphql",
    response_derives = "Debug"
)]
struct ResourceLastLogs;

pub async fn run_resources(args: ResourcesArgs, format: OutputFormat) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let token = auth::acquire_token(&client, &args.api_url).await?;
    let endpoint = auth::graphql_endpoint(&args.api_url);

    match args.command {
        ResourcesCommand::List {
            repository,
            environment,
        } => {
            list_resources(
                &client,
                &endpoint,
                &token,
                &repository,
                environment.as_deref(),
                format,
            )
            .await?;
        }
        ResourcesCommand::Logs {
            resource_qids,
            follow,
            latest,
        } => {
            if follow {
                stream_resource_logs(&endpoint, &token, &resource_qids, latest).await?;
            } else {
                print_resource_last_logs(
                    &client,
                    &endpoint,
                    &token,
                    &resource_qids,
                    latest,
                    format,
                )
                .await?;
            }
        }
    }

    Ok(())
}

#[derive(Serialize)]
struct ResourceListOutput {
    environment: String,
    r#type: String,
    name: String,
    owner: String,
    inputs: Option<serde_json::Value>,
    outputs: Option<serde_json::Value>,
}

async fn list_resources(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
    repository: &str,
    environment: Option<&str>,
    format: OutputFormat,
) -> anyhow::Result<()> {
    let (_, repository_name) = repo::parse_repository_path(repository)?;

    let data = auth::graphql_query::<ListRepositoryResources>(
        client,
        endpoint,
        token,
        list_repository_resources::Variables {},
        "resource list",
    )
    .await?;
    let repo = data
        .repositories
        .into_iter()
        .find(|r| r.name == repository_name)
        .ok_or_else(|| anyhow!("repository '{repository_name}' not found"))?;

    let environments: Vec<_> = repo
        .environments
        .into_iter()
        .filter(|env| environment.map(|filter| env.name == filter).unwrap_or(true))
        .collect();

    if let Some(env_name) = environment
        && environments.is_empty()
    {
        return Err(anyhow!("environment '{env_name}' not found"));
    }

    let resources: Vec<ResourceListOutput> = environments
        .into_iter()
        .flat_map(|env| {
            let env_name = env.name.clone();
            env.resources.into_iter().map(move |resource| {
                let owner = resource
                    .owner
                    .map(|o| o.id.chars().take(8).collect::<String>())
                    .unwrap_or_default();
                ResourceListOutput {
                    environment: env_name.clone(),
                    r#type: resource.type_,
                    name: resource.name,
                    owner,
                    inputs: resource.inputs,
                    outputs: resource.outputs,
                }
            })
        })
        .collect();

    match format {
        OutputFormat::Json => crate::output::print_json(&resources)?,
        OutputFormat::Text => {
            let mut table = crate::output::table("{:<}  {:<}  {:<}  {:<}  {:<}  {:<}");
            table.add_row(crate::output::row(vec![
                String::from("ENVIRONMENT"),
                String::from("TYPE"),
                String::from("NAME"),
                String::from("OWNER"),
                String::from("INPUTS"),
                String::from("OUTPUTS"),
            ]));
            for resource in resources {
                let inputs = resource
                    .inputs
                    .map(|v| serde_json::to_string(&v).unwrap_or_default())
                    .unwrap_or_default();
                let outputs = resource
                    .outputs
                    .map(|v| serde_json::to_string(&v).unwrap_or_default())
                    .unwrap_or_default();
                table.add_row(crate::output::row(vec![
                    resource.environment,
                    resource.r#type,
                    resource.name,
                    resource.owner,
                    inputs,
                    outputs,
                ]));
            }
            print!("{table}");
        }
    }

    Ok(())
}

/// Build a resource QID string from environment QID, resource type, and resource name.
fn build_resource_qid(env_qid: &str, resource_type: &str, resource_name: &str) -> String {
    format!("{env_qid}::{resource_type}:{resource_name}")
}

#[derive(Serialize)]
struct ResourceLogOutput {
    resource_qid: String,
    logs: Vec<LogOutput>,
}

#[derive(Serialize)]
struct LogOutput {
    severity: String,
    timestamp: String,
    message: String,
}

async fn print_resource_last_logs(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
    resource_qids: &[String],
    amount: i64,
    format: OutputFormat,
) -> anyhow::Result<()> {
    let data = auth::graphql_query::<ResourceLastLogs>(
        client,
        endpoint,
        token,
        resource_last_logs::Variables {
            amount: Some(amount),
        },
        "resource logs",
    )
    .await?;

    let mut results: Vec<ResourceLogOutput> = Vec::new();
    let mut missing: Vec<&str> = Vec::new();

    for qid in resource_qids {
        let mut found = false;
        for repo in &data.repositories {
            for env in &repo.environments {
                for resource in &env.resources {
                    let candidate = build_resource_qid(&env.qid, &resource.type_, &resource.name);
                    if candidate == *qid {
                        found = true;
                        results.push(ResourceLogOutput {
                            resource_qid: qid.clone(),
                            logs: resource
                                .last_logs
                                .iter()
                                .map(|l| LogOutput {
                                    severity: format!("{:?}", l.severity),
                                    timestamp: l.timestamp.clone(),
                                    message: l.message.clone(),
                                })
                                .collect(),
                        });
                    }
                }
            }
        }
        if !found {
            missing.push(qid);
        }
    }

    if !missing.is_empty() {
        return Err(anyhow!("resource(s) not found: {}", missing.join(", ")));
    }

    match format {
        OutputFormat::Json => crate::output::print_json(&results)?,
        OutputFormat::Text => {
            for (i, result) in results.iter().enumerate() {
                if i > 0 {
                    println!();
                }
                println!("==> {} <==", result.resource_qid);
                for log in &result.logs {
                    println!("[{}] [{}] {}", log.timestamp, log.severity, log.message);
                }
            }
        }
    }

    Ok(())
}

async fn stream_resource_logs(
    endpoint: &str,
    token: &str,
    resource_qids: &[String],
    initial_amount: i64,
) -> anyhow::Result<()> {
    let ws_endpoint = ws::graphql_ws_endpoint(endpoint)?;
    let (task_events_tx, mut task_events_rx) =
        tokio::sync::mpsc::unbounded_channel::<(String, anyhow::Result<()>)>();

    for resource_qid in resource_qids {
        let ws_endpoint = ws_endpoint.clone();
        let token = token.to_owned();
        let resource_qid = resource_qid.clone();
        let task_events_tx = task_events_tx.clone();
        tokio::spawn(async move {
            let qid = resource_qid.clone();
            let result = ws::stream_subscription(
                &ws_endpoint,
                &token,
                ws::SubscriptionParams {
                    subscription_id: &resource_qid,
                    query: include_str!("graphql/resource_logs_subscription.graphql"),
                    operation_name: "ResourceLogs",
                    variables: json!({
                        "resourceQid": resource_qid,
                        "initialAmount": initial_amount
                    }),
                    log_field_name: "resourceLogs",
                },
                |log| {
                    println!(
                        "[{}] [{}] [{}] {}",
                        qid, log.timestamp, log.severity, log.message
                    );
                },
            )
            .await;
            let _ = task_events_tx.send((resource_qid, result));
        });
    }

    loop {
        tokio::select! {
            signal = tokio::signal::ctrl_c() => {
                signal.context("failed to wait for ctrl-c")?;
                return Ok(());
            }
            event = task_events_rx.recv() => {
                let Some((resource_qid, result)) = event else {
                    return Ok(());
                };
                result.with_context(|| format!("log stream failed for resource {resource_qid}"))?;
            }
        }
    }
}

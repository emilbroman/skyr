use std::collections::HashMap;

use anyhow::{Context, anyhow};
use clap::{Args, Subcommand};
use graphql_client::GraphQLQuery;
use serde::Serialize;
use serde_json::json;

use crate::{auth, auth::JSON, context::Context as CliContext, output, output::OutputFormat, ws};

#[derive(Args, Debug)]
pub struct ResourcesArgs {
    #[command(subcommand)]
    command: ResourcesCommand,
}

#[derive(Subcommand, Debug)]
enum ResourcesCommand {
    /// List resources for the current repository, optionally filtered by env.
    List,
    /// Show or stream logs for one or more resources (interlaced).
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
    /// Request deletion of a single resource. The server enqueues a Destroy
    /// message for the RTE worker pool and returns immediately; the
    /// plugin-side delete runs asynchronously.
    Delete {
        /// Resource ID in `Type:name` form (e.g. `Std/Random.Int:seed`).
        resource: String,
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

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "../api/schema.graphql",
    query_path = "src/graphql/delete_resource.graphql",
    response_derives = "Debug"
)]
struct DeleteResource;

pub async fn run_resources(args: ResourcesArgs, ctx: &CliContext) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let token = auth::acquire_token(&client, ctx.api_url()).await?;
    let endpoint = auth::graphql_endpoint(ctx.api_url());

    match args.command {
        ResourcesCommand::List => {
            let organization = ctx.org()?.to_owned();
            let repository = ctx.repo()?.to_owned();
            let environment = ctx.env().ok().map(str::to_owned);
            list_resources(
                &client,
                &endpoint,
                &token,
                &organization,
                &repository,
                environment.as_deref(),
                ctx.format,
            )
            .await
        }
        ResourcesCommand::Logs {
            resource_qids,
            follow,
            latest,
        } => {
            if follow {
                stream_resource_logs(&endpoint, &token, &resource_qids, latest).await
            } else {
                let organization = ctx.org()?.to_owned();
                let repository = ctx.repo()?.to_owned();
                print_resource_last_logs(
                    &client,
                    &endpoint,
                    &token,
                    &organization,
                    &repository,
                    &resource_qids,
                    latest,
                    ctx.format,
                )
                .await
            }
        }
        ResourcesCommand::Delete { resource } => {
            let organization = ctx.org()?.to_owned();
            let repository = ctx.repo()?.to_owned();
            let environment = ctx.env()?.to_owned();
            delete_resource(
                &client,
                &endpoint,
                &token,
                &organization,
                &repository,
                &environment,
                &resource,
                ctx.format,
            )
            .await
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn delete_resource(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
    organization: &str,
    repository: &str,
    environment: &str,
    resource: &str,
    format: OutputFormat,
) -> anyhow::Result<()> {
    auth::graphql_query::<DeleteResource>(
        client,
        endpoint,
        token,
        delete_resource::Variables {
            organization: organization.to_owned(),
            repository: repository.to_owned(),
            environment: environment.to_owned(),
            resource: resource.to_owned(),
        },
        "resource delete",
    )
    .await?;

    #[derive(Serialize)]
    struct DeleteResourceOutput {
        organization: String,
        repository: String,
        environment: String,
        resource: String,
    }

    let output = DeleteResourceOutput {
        organization: organization.to_owned(),
        repository: repository.to_owned(),
        environment: environment.to_owned(),
        resource: resource.to_owned(),
    };

    match format {
        OutputFormat::Json => output::print_json(&output)?,
        OutputFormat::Text => {
            println!(
                "delete requested for {}/{}::{}::{}",
                output.organization, output.repository, output.environment, output.resource
            );
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

#[allow(clippy::too_many_arguments)]
async fn list_resources(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
    organization: &str,
    repository: &str,
    environment: Option<&str>,
    format: OutputFormat,
) -> anyhow::Result<()> {
    let data = auth::graphql_query::<ListRepositoryResources>(
        client,
        endpoint,
        token,
        list_repository_resources::Variables {
            organization: organization.to_owned(),
            repository: repository.to_owned(),
        },
        "resource list",
    )
    .await?;

    let environments: Vec<_> = data
        .organization
        .repository
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
        OutputFormat::Json => output::print_json(&resources)?,
        OutputFormat::Text => {
            let mut table = output::table("{:<}  {:<}  {:<}  {:<}  {:<}  {:<}");
            table.add_row(output::row(vec![
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
                table.add_row(output::row(vec![
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
    logs: Vec<output::LogOutput>,
}

#[allow(clippy::too_many_arguments)]
async fn print_resource_last_logs(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
    organization: &str,
    repository: &str,
    resource_qids: &[String],
    amount: i64,
    format: OutputFormat,
) -> anyhow::Result<()> {
    let data = auth::graphql_query::<ResourceLastLogs>(
        client,
        endpoint,
        token,
        resource_last_logs::Variables {
            organization: organization.to_owned(),
            repository: repository.to_owned(),
            amount: Some(amount),
        },
        "resource logs",
    )
    .await?;

    let mut resource_logs: HashMap<String, Vec<output::LogOutput>> = HashMap::new();
    for env in &data.organization.repository.environments {
        for resource in &env.resources {
            let qid = build_resource_qid(&env.qid, &resource.type_, &resource.name);
            resource_logs.insert(
                qid,
                resource
                    .last_logs
                    .iter()
                    .map(|l| output::LogOutput {
                        severity: format!("{:?}", l.severity),
                        timestamp: l.timestamp.clone(),
                        message: l.message.clone(),
                    })
                    .collect(),
            );
        }
    }

    let mut results: Vec<ResourceLogOutput> = Vec::new();
    let mut missing: Vec<&str> = Vec::new();

    for qid in resource_qids {
        if let Some(logs) = resource_logs.remove(qid.as_str()) {
            results.push(ResourceLogOutput {
                resource_qid: qid.clone(),
                logs,
            });
        } else {
            missing.push(qid);
        }
    }

    if !missing.is_empty() {
        return Err(anyhow!("resource(s) not found: {}", missing.join(", ")));
    }

    match format {
        OutputFormat::Json => output::print_json(&results)?,
        OutputFormat::Text => {
            let groups: Vec<_> = results
                .iter()
                .map(|r| (r.resource_qid.as_str(), r.logs.as_slice()))
                .collect();
            output::print_logs_text(&groups);
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

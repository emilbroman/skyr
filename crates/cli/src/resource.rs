use anyhow::{Context, anyhow};
use clap::{Args, Subcommand};
use futures_util::{SinkExt, StreamExt};
use graphql_client::GraphQLQuery;
use serde::Serialize;
use serde_json::json;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::{auth, deployment, output::OutputFormat, repo};

#[allow(clippy::upper_case_acronyms)]
type JSON = serde_json::Value;

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
    /// Stream logs for one or more resources (interlaced)
    Logs {
        /// Resource QIDs (e.g. org/repo::env::Type:name), at least one required
        #[arg(required = true, num_args = 1..)]
        resource_qids: Vec<String>,
        /// Number of initial log entries to fetch per resource
        #[arg(long, default_value = "200")]
        initial: i64,
    },
}

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "../api/schema.graphql",
    query_path = "src/graphql/list_repository_resources.graphql",
    response_derives = "Debug"
)]
struct ListRepositoryResources;

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
            initial,
        } => {
            stream_resource_logs(&endpoint, &token, &resource_qids, initial).await?;
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

    let body = ListRepositoryResources::build_query(list_repository_resources::Variables {});
    let response = client
        .post(endpoint)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&body)
        .send()
        .await
        .context("failed to send resources query")?;
    let response: graphql_client::Response<list_repository_resources::ResponseData> = response
        .json()
        .await
        .context("failed to decode resources response")?;
    let data = auth::graphql_response_data(response, "resource list")?;
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

async fn stream_resource_logs(
    endpoint: &str,
    token: &str,
    resource_qids: &[String],
    initial_amount: i64,
) -> anyhow::Result<()> {
    let ws_endpoint = deployment::graphql_ws_endpoint(endpoint)?;
    let (task_events_tx, mut task_events_rx) =
        tokio::sync::mpsc::unbounded_channel::<(String, anyhow::Result<()>)>();

    for resource_qid in resource_qids {
        let ws_endpoint = ws_endpoint.clone();
        let token = token.to_owned();
        let resource_qid = resource_qid.clone();
        let task_events_tx = task_events_tx.clone();
        tokio::spawn(async move {
            let result =
                stream_single_resource(&ws_endpoint, &token, resource_qid.clone(), initial_amount)
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

async fn stream_single_resource(
    ws_endpoint: &str,
    token: &str,
    resource_qid: String,
    initial_amount: i64,
) -> anyhow::Result<()> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let mut request = ws_endpoint
        .into_client_request()
        .context("failed to build websocket request")?;
    request.headers_mut().insert(
        reqwest::header::AUTHORIZATION,
        reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
            .context("failed to format authorization header")?,
    );
    request.headers_mut().insert(
        reqwest::header::SEC_WEBSOCKET_PROTOCOL,
        reqwest::header::HeaderValue::from_static("graphql-transport-ws"),
    );

    let (ws_stream, _) = connect_async(request)
        .await
        .with_context(|| format!("failed to connect websocket at {ws_endpoint}"))?;
    let (mut write, mut read) = ws_stream.split();

    write
        .send(Message::Text(
            json!({
                "type": "connection_init"
            })
            .to_string(),
        ))
        .await
        .with_context(|| {
            format!(
                "failed to send graphql websocket connection init for resource {}",
                resource_qid
            )
        })?;

    deployment::wait_for_connection_ack(&mut read, &mut write).await?;

    write
        .send(Message::Text(
            json!({
                "id": resource_qid,
                "type": "subscribe",
                "payload": {
                    "query": include_str!("graphql/resource_logs_subscription.graphql"),
                    "variables": {
                        "resourceQid": resource_qid,
                        "initialAmount": initial_amount
                    },
                    "operationName": "ResourceLogs"
                }
            })
            .to_string(),
        ))
        .await
        .context("failed to send resource logs subscription")?;

    while let Some(message) = read.next().await {
        let message = message.context("failed to read websocket message")?;
        match message {
            Message::Text(text) => {
                if let Some(log) = deployment::decode_subscription_log(&text, "resourceLogs")? {
                    println!(
                        "[{}] [{}] [{}] {}",
                        resource_qid, log.timestamp, log.severity, log.message
                    );
                }
            }
            Message::Binary(_) => {}
            Message::Ping(payload) => {
                write
                    .send(Message::Pong(payload))
                    .await
                    .context("failed to send websocket pong")?;
            }
            Message::Pong(_) => {}
            Message::Close(_) => {
                break;
            }
            Message::Frame(_) => {}
        }
    }

    Ok(())
}

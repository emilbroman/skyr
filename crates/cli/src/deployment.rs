use anyhow::{Context, anyhow};
use clap::{Args, Subcommand};
use futures_util::{SinkExt, StreamExt};
use graphql_client::GraphQLQuery;
use serde::Serialize;
use serde_json::json;
use std::{collections::BTreeSet, time::Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::{auth, output::OutputFormat, repo};

#[allow(clippy::upper_case_acronyms)]
type JSON = serde_json::Value;

#[derive(Args, Debug)]
pub struct DeploymentsArgs {
    #[command(subcommand)]
    command: DeploymentsCommand,
    #[arg(long, default_value = "localhost:8080")]
    api_url: String,
}

#[derive(Subcommand, Debug)]
enum DeploymentsCommand {
    List { repository: String },
    Logs { repository: String },
}

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "../api/schema.graphql",
    query_path = "src/graphql/list_repository_deployments.graphql",
    response_derives = "Debug"
)]
struct ListRepositoryDeployments;

pub async fn run_deployments(args: DeploymentsArgs, format: OutputFormat) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let token = auth::acquire_token(&client, &args.api_url).await?;
    let endpoint = auth::graphql_endpoint(&args.api_url);

    match args.command {
        DeploymentsCommand::List { repository } => {
            list_deployments(&client, &endpoint, &token, &repository, format).await?;
        }
        DeploymentsCommand::Logs { repository } => {
            stream_deployment_logs(&client, &endpoint, &token, &repository).await?;
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
    id: String,
    inputs: Option<serde_json::Value>,
    outputs: Option<serde_json::Value>,
    dependencies: Vec<ResourceDependencyOutput>,
}

#[derive(Serialize)]
struct ResourceDependencyOutput {
    r#type: String,
    id: String,
}

#[derive(Debug)]
struct SubscriptionLog {
    severity: String,
    timestamp: String,
    message: String,
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
                    r#ref: deployment.ref_.to_owned(),
                    commit: deployment.commit.to_owned(),
                    created_at: deployment.created_at.to_owned(),
                    state: format!("{:?}", deployment.state),
                    resources: deployment
                        .resources
                        .into_iter()
                        .map(|resource| ResourceOutput {
                            r#type: resource.type_.to_owned(),
                            id: resource.id,
                            inputs: resource.inputs,
                            outputs: resource.outputs,
                            dependencies: resource
                                .dependencies
                                .into_iter()
                                .map(|dependency| ResourceDependencyOutput {
                                    r#type: dependency.type_,
                                    id: dependency.id,
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

async fn stream_deployment_logs(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
    repository: &str,
) -> anyhow::Result<()> {
    let (_, repository_name) = repo::parse_repository_path(repository)?;
    let ws_endpoint = graphql_ws_endpoint(endpoint)?;
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
                tokio::spawn(async move {
                    let result =
                        stream_single_environment(&ws_endpoint, &token, environment_qid.clone())
                            .await;
                    let _ = task_events_tx.send((environment_qid, result));
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

async fn stream_single_environment(
    ws_endpoint: &str,
    token: &str,
    environment_qid: String,
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
                "failed to send graphql websocket connection init for environment {}",
                environment_qid
            )
        })?;

    wait_for_connection_ack(&mut read, &mut write).await?;

    write
        .send(Message::Text(
            json!({
                "id": environment_qid,
                "type": "subscribe",
                "payload": {
                    "query": include_str!("graphql/deployment_logs_subscription.graphql"),
                    "variables": {
                        "environmentQid": environment_qid,
                        "initialAmount": 200
                    },
                    "operationName": "EnvironmentLogs"
                }
            })
            .to_string(),
        ))
        .await
        .context("failed to send environment logs subscription")?;

    while let Some(message) = read.next().await {
        let message = message.context("failed to read websocket message")?;
        match message {
            Message::Text(text) => {
                if let Some(log) = decode_log_message(&text)? {
                    println!(
                        "[{}] [{}] [{}] {}",
                        environment_qid, log.timestamp, log.severity, log.message
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

async fn wait_for_connection_ack<Read, Write>(
    read: &mut Read,
    write: &mut Write,
) -> anyhow::Result<()>
where
    Read:
        futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
    Write: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    while let Some(message) = read.next().await {
        let message = message.context("failed to read connection ack message")?;
        match message {
            Message::Text(text) => {
                let value: serde_json::Value = serde_json::from_str(&text)
                    .with_context(|| format!("failed to decode websocket message: {text}"))?;
                match value
                    .get("type")
                    .and_then(|message_type| message_type.as_str())
                {
                    Some("connection_ack") => return Ok(()),
                    Some("ping") => {
                        write
                            .send(Message::Text(json!({ "type": "pong" }).to_string()))
                            .await
                            .context("failed to send graphql ping response")?;
                    }
                    Some("connection_error") => {
                        return Err(anyhow!(
                            "graphql websocket connection rejected: {}",
                            value
                                .get("payload")
                                .map(|payload| payload.to_string())
                                .unwrap_or_else(|| String::from("<empty payload>"))
                        ));
                    }
                    _ => {}
                }
            }
            Message::Ping(payload) => {
                write
                    .send(Message::Pong(payload))
                    .await
                    .context("failed to send websocket pong")?;
            }
            Message::Close(_) => return Err(anyhow!("websocket closed before connection ack")),
            Message::Binary(_) | Message::Pong(_) | Message::Frame(_) => {}
        }
    }

    Err(anyhow!("websocket closed before connection ack"))
}

fn decode_log_message(text: &str) -> anyhow::Result<Option<SubscriptionLog>> {
    let value: serde_json::Value =
        serde_json::from_str(text).with_context(|| format!("failed to decode message: {text}"))?;

    let Some(message_type) = value
        .get("type")
        .and_then(|message_type| message_type.as_str())
    else {
        return Ok(None);
    };

    match message_type {
        "next" => {
            let payload = value
                .get("payload")
                .ok_or_else(|| anyhow!("subscription message missing payload"))?;
            if let Some(errors) = payload.get("errors") {
                return Err(anyhow!("subscription returned errors: {errors}"));
            }

            let log = payload
                .get("data")
                .and_then(|data| data.get("environmentLogs"))
                .ok_or_else(|| anyhow!("subscription message missing environmentLogs"))?;

            let severity = log
                .get("severity")
                .and_then(|severity| severity.as_str())
                .ok_or_else(|| anyhow!("log entry missing severity"))?;
            let timestamp = log
                .get("timestamp")
                .and_then(|timestamp| timestamp.as_str())
                .ok_or_else(|| anyhow!("log entry missing timestamp"))?;
            let message = log
                .get("message")
                .and_then(|message| message.as_str())
                .ok_or_else(|| anyhow!("log entry missing message"))?;

            Ok(Some(SubscriptionLog {
                severity: severity.to_string(),
                timestamp: timestamp.to_string(),
                message: message.to_string(),
            }))
        }
        "error" => {
            let payload = value
                .get("payload")
                .map(|payload| payload.to_string())
                .unwrap_or_else(|| String::from("<empty payload>"));
            Err(anyhow!("subscription returned error: {payload}"))
        }
        "complete" | "ka" | "connection_ack" | "pong" | "ping" => Ok(None),
        other => Err(anyhow!("unsupported subscription message type: {other}")),
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
    let response = client
        .post(endpoint)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&body)
        .send()
        .await
        .context("failed to send deployments query")?;
    let response: graphql_client::Response<list_repository_deployments::ResponseData> = response
        .json()
        .await
        .context("failed to decode deployments response")?;

    if let Some(errors) = response.errors {
        return Err(anyhow!(
            "deployment list failed: {}",
            errors
                .iter()
                .map(|e| e.message.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }

    let data = response
        .data
        .context("deployments response did not include data")?;
    let repository = data
        .repositories
        .into_iter()
        .find(|repo| repo.name == repository_name)
        .ok_or_else(|| anyhow!("repository '{repository_name}' not found"))?;

    Ok(repository.environments)
}

fn graphql_ws_endpoint(graphql_endpoint: &str) -> anyhow::Result<String> {
    let ws_endpoint = if let Some(rest) = graphql_endpoint.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = graphql_endpoint.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if graphql_endpoint.starts_with("ws://") || graphql_endpoint.starts_with("wss://") {
        graphql_endpoint.to_string()
    } else {
        return Err(anyhow!(
            "unsupported graphql endpoint scheme for websocket: {graphql_endpoint}"
        ));
    };

    Ok(ws_endpoint)
}

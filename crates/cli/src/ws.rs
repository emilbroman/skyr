//! Shared WebSocket streaming logic for GraphQL subscriptions.

use anyhow::{Context, anyhow};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::auth;

/// A decoded log entry from a GraphQL subscription.
#[derive(Debug)]
pub(crate) struct SubscriptionLog {
    pub(crate) severity: String,
    pub(crate) timestamp: String,
    pub(crate) message: String,
}

/// Parameters describing a GraphQL subscription to stream.
pub(crate) struct SubscriptionParams<'a> {
    pub(crate) subscription_id: &'a str,
    pub(crate) query: &'a str,
    pub(crate) operation_name: &'a str,
    pub(crate) variables: serde_json::Value,
    pub(crate) log_field_name: &'a str,
}

/// Open a WebSocket connection to the given endpoint with authentication,
/// perform the graphql-transport-ws handshake (connection_init + ack),
/// send a subscription message, and invoke `on_log` for each log entry received.
pub(crate) async fn stream_subscription(
    ws_endpoint: &str,
    token: &str,
    params: SubscriptionParams<'_>,
    mut on_log: impl FnMut(&SubscriptionLog),
) -> anyhow::Result<()> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let mut request = ws_endpoint
        .into_client_request()
        .context("failed to build websocket request")?;
    let header_value = auth::bearer_header_value(token)?;
    request
        .headers_mut()
        .insert(reqwest::header::AUTHORIZATION, header_value);
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
            json!({ "type": "connection_init" }).to_string(),
        ))
        .await
        .with_context(|| {
            format!(
                "failed to send graphql websocket connection init for {}",
                params.subscription_id
            )
        })?;

    wait_for_connection_ack(&mut read, &mut write).await?;

    write
        .send(Message::Text(
            json!({
                "id": params.subscription_id,
                "type": "subscribe",
                "payload": {
                    "query": params.query,
                    "variables": params.variables,
                    "operationName": params.operation_name
                }
            })
            .to_string(),
        ))
        .await
        .with_context(|| format!("failed to send {} subscription", params.operation_name))?;

    while let Some(message) = read.next().await {
        let message = message.context("failed to read websocket message")?;
        match message {
            Message::Text(text) => {
                if let Some(log) = decode_subscription_log(&text, params.log_field_name)? {
                    on_log(&log);
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

fn decode_subscription_log(
    text: &str,
    field_name: &str,
) -> anyhow::Result<Option<SubscriptionLog>> {
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
                .and_then(|data| data.get(field_name))
                .ok_or_else(|| anyhow!("subscription message missing {field_name}"))?;

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

/// Convert an HTTP(S) GraphQL endpoint to its WebSocket equivalent.
pub(crate) fn graphql_ws_endpoint(graphql_endpoint: &str) -> anyhow::Result<String> {
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

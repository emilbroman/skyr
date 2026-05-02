use std::sync::Arc;

use axum::extract::ws::{Message as WsMessage, WebSocket};
use futures_util::{SinkExt, StreamExt};
use juniper::Value;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

use crate::json_scalar::{graphql_value_to_json, serialize_execution_errors};
use crate::region_keys::RegionKeyCache;
use crate::{AuthOutcome, Context, Schema, authenticate_token};

pub(crate) async fn graphql_ws_connection(
    socket: WebSocket,
    schema: Arc<Schema>,
    mut context: Context,
    region_keys: RegionKeyCache,
) {
    use std::collections::HashMap;

    let (mut ws_sink, mut ws_stream) = socket.split();

    // All outgoing frames funnel through this channel so multiple concurrent
    // subscription tasks can write to the socket without contending.
    let (outgoing_tx, mut outgoing_rx) = mpsc::unbounded_channel::<WsMessage>();

    let writer = tokio::spawn(async move {
        while let Some(msg) = outgoing_rx.recv().await {
            if ws_sink.send(msg).await.is_err() {
                break;
            }
        }
        let _ = ws_sink.close().await;
    });

    let send_outgoing = |value: serde_json::Value| -> bool {
        outgoing_tx.send(WsMessage::Text(value.to_string())).is_ok()
    };

    let mut initialized = false;
    let mut subscriptions: HashMap<String, AbortHandle> = HashMap::new();

    while let Some(message) = ws_stream.next().await {
        let message = match message {
            Ok(message) => message,
            Err(error) => {
                tracing::warn!("GraphQL websocket receive error: {}", error);
                break;
            }
        };

        match message {
            WsMessage::Text(text) => {
                let payload: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(payload) => payload,
                    Err(error) => {
                        tracing::warn!("GraphQL websocket invalid json message: {}", error);
                        if !send_outgoing(serde_json::json!({
                            "type": "error",
                            "payload": [{
                                "message": format!("invalid websocket message: {error}")
                            }]
                        })) {
                            break;
                        }
                        continue;
                    }
                };

                let Some(message_type) = payload
                    .get("type")
                    .and_then(|message_type| message_type.as_str())
                else {
                    if !send_outgoing(serde_json::json!({
                        "type": "error",
                        "payload": [{
                            "message": "missing websocket message type"
                        }]
                    })) {
                        break;
                    }
                    continue;
                };

                match message_type {
                    "connection_init" => {
                        // Support auth via connection_init payload for browser clients
                        // that cannot set custom HTTP headers on WebSocket upgrade.
                        if context.authenticated_user.is_none()
                            && let Some(token) = payload
                                .get("payload")
                                .and_then(|p| p.get("Authorization"))
                                .and_then(|v| v.as_str())
                                .and_then(|v| v.strip_prefix("Bearer "))
                        {
                            match authenticate_token(token, &region_keys).await {
                                AuthOutcome::Authenticated(user) => {
                                    context.authenticated_user = Some(user);
                                }
                                AuthOutcome::Invalid | AuthOutcome::Expired => {
                                    tracing::debug!("Invalid token in connection_init payload");
                                }
                                AuthOutcome::Internal => {
                                    tracing::error!(
                                        "Internal error verifying token from connection_init"
                                    );
                                }
                            }
                        }

                        initialized = true;
                        if !send_outgoing(serde_json::json!({
                            "type": "connection_ack"
                        })) {
                            break;
                        }
                    }
                    "ping" => {
                        if !send_outgoing(serde_json::json!({
                            "type": "pong"
                        })) {
                            break;
                        }
                    }
                    "subscribe" => {
                        if !initialized {
                            if !send_outgoing(serde_json::json!({
                                "type": "error",
                                "payload": [{
                                    "message": "connection_init must be sent before subscribe"
                                }]
                            })) {
                                break;
                            }
                            continue;
                        }

                        let Some(subscription_id) = payload
                            .get("id")
                            .and_then(|id| id.as_str())
                            .map(ToOwned::to_owned)
                        else {
                            if !send_outgoing(serde_json::json!({
                                "type": "error",
                                "payload": [{
                                    "message": "subscribe message missing id"
                                }]
                            })) {
                                break;
                            }
                            continue;
                        };

                        let Some(request_payload) = payload.get("payload").cloned() else {
                            if !send_outgoing(serde_json::json!({
                                "id": subscription_id,
                                "type": "error",
                                "payload": [{
                                    "message": "subscribe message missing payload"
                                }]
                            })) {
                                break;
                            }
                            continue;
                        };

                        let request: juniper::http::GraphQLRequest =
                            match serde_json::from_value(request_payload) {
                                Ok(request) => request,
                                Err(error) => {
                                    if !send_outgoing(serde_json::json!({
                                        "id": subscription_id,
                                        "type": "error",
                                        "payload": [{
                                            "message": format!("invalid subscribe payload: {error}")
                                        }]
                                    })) {
                                        break;
                                    }
                                    continue;
                                }
                            };

                        // If the client reuses a subscription id, treat it as a
                        // resubscribe: abort the previous task before starting a new one.
                        if let Some(existing) = subscriptions.remove(&subscription_id) {
                            existing.abort();
                        }

                        let task = tokio::spawn(run_subscription(
                            subscription_id.clone(),
                            request,
                            Arc::clone(&schema),
                            context.clone(),
                            outgoing_tx.clone(),
                        ));
                        subscriptions.insert(subscription_id, task.abort_handle());
                    }
                    "complete" => {
                        if let Some(id) = payload.get("id").and_then(|id| id.as_str())
                            && let Some(handle) = subscriptions.remove(id)
                        {
                            handle.abort();
                        }
                    }
                    "pong" => {}
                    other => {
                        if !send_outgoing(serde_json::json!({
                            "type": "error",
                            "payload": [{
                                "message": format!("unsupported websocket message type: {other}")
                            }]
                        })) {
                            break;
                        }
                    }
                }
            }
            WsMessage::Ping(bytes) => {
                if outgoing_tx.send(WsMessage::Pong(bytes)).is_err() {
                    break;
                }
            }
            WsMessage::Pong(_) => {}
            WsMessage::Close(_) => break,
            WsMessage::Binary(_) => {}
        }
    }

    for (_, handle) in subscriptions.drain() {
        handle.abort();
    }
    drop(outgoing_tx);
    let _ = writer.await;
}

async fn run_subscription(
    subscription_id: String,
    request: juniper::http::GraphQLRequest,
    schema: Arc<Schema>,
    context: Context,
    outgoing: mpsc::UnboundedSender<WsMessage>,
) {
    let send = |value: serde_json::Value| -> bool {
        outgoing.send(WsMessage::Text(value.to_string())).is_ok()
    };

    let stream_result = juniper::http::resolve_into_stream(&request, &schema, &context).await;

    let (subscription_value, initial_errors) = match stream_result {
        Ok(result) => result,
        Err(error) => {
            let _ = send(serde_json::json!({
                "id": subscription_id,
                "type": "error",
                "payload": [{
                    "message": format!("{error}")
                }]
            }));
            return;
        }
    };

    if !initial_errors.is_empty() {
        let _ = send(serde_json::json!({
            "id": subscription_id,
            "type": "error",
            "payload": serialize_execution_errors(&initial_errors)
        }));
        return;
    }

    let Some(fields) = subscription_value.into_object() else {
        let _ = send(serde_json::json!({
            "id": subscription_id,
            "type": "error",
            "payload": [{
                "message": "subscription did not return a stream field"
            }]
        }));
        return;
    };
    let Some((field_name, field_value)) = fields.into_iter().next() else {
        let _ = send(serde_json::json!({
            "id": subscription_id,
            "type": "error",
            "payload": [{
                "message": "subscription did not return any stream fields"
            }]
        }));
        return;
    };
    let Value::Scalar(mut stream) = field_value else {
        let _ = send(serde_json::json!({
            "id": subscription_id,
            "type": "error",
            "payload": [{
                "message": "subscription field was not a stream"
            }]
        }));
        return;
    };

    while let Some(item) = stream.next().await {
        let event = match item {
            Ok(value) => serde_json::json!({
                "id": subscription_id,
                "type": "next",
                "payload": {
                    "data": {
                        field_name.clone(): graphql_value_to_json(&value)
                    }
                }
            }),
            Err(error) => serde_json::json!({
                "id": subscription_id,
                "type": "next",
                "payload": {
                    "errors": serialize_execution_errors(std::slice::from_ref(&error))
                }
            }),
        };

        if !send(event) {
            return;
        }
    }

    let _ = send(serde_json::json!({
        "id": subscription_id,
        "type": "complete"
    }));
}

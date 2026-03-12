use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Number(i64),
    String(String),
}

#[derive(Debug)]
pub enum IncomingMessage {
    Request {
        id: RequestId,
        method: String,
        params: serde_json::Value,
    },
    Notification {
        method: String,
        params: serde_json::Value,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum OutgoingMessage {
    Response(Response),
    Notification(NotificationMessage),
}

#[derive(Debug, Clone, Serialize)]
pub struct Response {
    jsonrpc: &'static str,
    id: RequestId,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ResponseError>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResponseError {
    code: i32,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NotificationMessage {
    jsonrpc: &'static str,
    method: String,
    params: serde_json::Value,
}

impl OutgoingMessage {
    pub fn response<T: Serialize>(id: RequestId, result: T) -> Self {
        OutgoingMessage::Response(Response {
            jsonrpc: "2.0",
            id,
            result: Some(serde_json::to_value(result).unwrap()),
            error: None,
        })
    }

    pub fn error(id: RequestId, code: i32, message: String) -> Self {
        OutgoingMessage::Response(Response {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(ResponseError { code, message }),
        })
    }

    pub fn notification<T: Serialize>(method: &str, params: T) -> Self {
        OutgoingMessage::Notification(NotificationMessage {
            jsonrpc: "2.0",
            method: method.to_string(),
            params: serde_json::to_value(params).unwrap(),
        })
    }
}

#[derive(Debug, Deserialize)]
struct RawMessage {
    #[allow(dead_code)]
    jsonrpc: String,
    id: Option<RequestId>,
    method: Option<String>,
    #[serde(default)]
    params: serde_json::Value,
}

impl IncomingMessage {
    pub fn parse(json: &str) -> Result<Self, serde_json::Error> {
        let raw: RawMessage = serde_json::from_str(json)?;

        let method = raw
            .method
            .ok_or_else(|| serde::de::Error::missing_field("method"))?;

        match raw.id {
            Some(id) => Ok(IncomingMessage::Request {
                id,
                method,
                params: raw.params,
            }),
            None => Ok(IncomingMessage::Notification {
                method,
                params: raw.params,
            }),
        }
    }
}

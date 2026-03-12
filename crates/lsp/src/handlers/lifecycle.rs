use lsp_types as lsp;

use crate::server::{OutgoingMessage, RequestId};

pub fn initialize(id: RequestId, params: serde_json::Value) -> Vec<OutgoingMessage> {
    let _params: lsp::InitializeParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(err) => {
            return vec![OutgoingMessage::error(
                id,
                -32602,
                format!("Invalid initialize params: {err}"),
            )];
        }
    };

    let capabilities = lsp::ServerCapabilities {
        text_document_sync: Some(lsp::TextDocumentSyncCapability::Options(
            lsp::TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(lsp::TextDocumentSyncKind::FULL),
                save: Some(lsp::TextDocumentSyncSaveOptions::SaveOptions(
                    lsp::SaveOptions {
                        include_text: Some(false),
                    },
                )),
                ..Default::default()
            },
        )),
        ..Default::default()
    };

    let result = lsp::InitializeResult {
        capabilities,
        server_info: Some(lsp::ServerInfo {
            name: "scl-language-server".to_string(),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        }),
    };

    vec![OutgoingMessage::response(
        id,
        serde_json::to_value(result).unwrap(),
    )]
}

pub fn shutdown(id: RequestId) -> Vec<OutgoingMessage> {
    vec![OutgoingMessage::response(id, serde_json::Value::Null)]
}

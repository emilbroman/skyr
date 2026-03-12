mod convert;
mod document;
mod handlers;
mod helpers;
mod overlay;
mod query;
mod transport;

pub use transport::{IncomingMessage, OutgoingMessage, RequestId};

use std::path::PathBuf;
use std::sync::Arc;

use lsp_types::{notification::Notification, request::Request};
use sclc::{Program, SourceRepo};
use tokio::sync::Mutex;

use document::DocumentCache;
use overlay::OverlaySource;

pub struct LanguageServer<S> {
    documents: Arc<Mutex<DocumentCache>>,
    source_factory: Box<dyn Fn() -> S + Send + Sync>,
    root_path: Option<PathBuf>,
    initialized: bool,
    shutdown_requested: bool,
    exited: bool,
    last_program: Option<Program<OverlaySource<S>>>,
}

impl<S: SourceRepo + 'static> LanguageServer<S> {
    pub fn new<F>(source_factory: F) -> Self
    where
        F: Fn() -> S + Send + Sync + 'static,
    {
        Self {
            documents: Arc::new(Mutex::new(DocumentCache::new())),
            source_factory: Box::new(source_factory),
            root_path: None,
            initialized: false,
            shutdown_requested: false,
            exited: false,
            last_program: None,
        }
    }

    pub async fn handle(&mut self, msg: IncomingMessage) -> Vec<OutgoingMessage> {
        match msg {
            IncomingMessage::Request { id, method, params } => {
                self.handle_request(id, &method, params).await
            }
            IncomingMessage::Notification { method, params } => {
                self.handle_notification(&method, params).await
            }
        }
    }

    async fn handle_request(
        &mut self,
        id: RequestId,
        method: &str,
        params: serde_json::Value,
    ) -> Vec<OutgoingMessage> {
        // Allow `initialize` and `shutdown` before initialization; reject everything else.
        if !self.initialized
            && method != lsp_types::request::Initialize::METHOD
            && method != lsp_types::request::Shutdown::METHOD
        {
            return vec![OutgoingMessage::error(
                id,
                -32002,
                "Server not yet initialized".to_string(),
            )];
        }

        match method {
            lsp_types::request::Initialize::METHOD => {
                let params: lsp_types::InitializeParams = match serde_json::from_value(params) {
                    Ok(p) => p,
                    Err(e) => {
                        return vec![OutgoingMessage::error(
                            id,
                            -32602,
                            format!("Invalid params: {}", e),
                        )];
                    }
                };
                let result = handlers::lifecycle::handle_initialize(self, params);
                vec![OutgoingMessage::response(id, result)]
            }
            lsp_types::request::Shutdown::METHOD => {
                handlers::lifecycle::handle_shutdown(self);
                vec![OutgoingMessage::response(id, serde_json::Value::Null)]
            }
            lsp_types::request::DocumentSymbolRequest::METHOD => {
                let params: lsp_types::DocumentSymbolParams = match serde_json::from_value(params) {
                    Ok(p) => p,
                    Err(e) => {
                        return vec![OutgoingMessage::error(
                            id,
                            -32602,
                            format!("Invalid params: {}", e),
                        )];
                    }
                };
                handlers::navigation::handle_document_symbol(self, id, params).await
            }
            lsp_types::request::GotoDefinition::METHOD => {
                let params: lsp_types::GotoDefinitionParams = match serde_json::from_value(params) {
                    Ok(p) => p,
                    Err(e) => {
                        return vec![OutgoingMessage::error(
                            id,
                            -32602,
                            format!("Invalid params: {}", e),
                        )];
                    }
                };
                handlers::navigation::handle_goto_definition(self, id, params).await
            }
            lsp_types::request::HoverRequest::METHOD => {
                let params: lsp_types::HoverParams = match serde_json::from_value(params) {
                    Ok(p) => p,
                    Err(e) => {
                        return vec![OutgoingMessage::error(
                            id,
                            -32602,
                            format!("Invalid params: {}", e),
                        )];
                    }
                };
                handlers::navigation::handle_hover(self, id, params).await
            }
            lsp_types::request::References::METHOD => {
                let params: lsp_types::ReferenceParams = match serde_json::from_value(params) {
                    Ok(p) => p,
                    Err(e) => {
                        return vec![OutgoingMessage::error(
                            id,
                            -32602,
                            format!("Invalid params: {}", e),
                        )];
                    }
                };
                handlers::navigation::handle_references(self, id, params).await
            }
            lsp_types::request::Rename::METHOD => {
                let params: lsp_types::RenameParams = match serde_json::from_value(params) {
                    Ok(p) => p,
                    Err(e) => {
                        return vec![OutgoingMessage::error(
                            id,
                            -32602,
                            format!("Invalid params: {}", e),
                        )];
                    }
                };
                handlers::rename::handle_rename(self, id, params).await
            }
            lsp_types::request::PrepareRenameRequest::METHOD => {
                let params: lsp_types::TextDocumentPositionParams =
                    match serde_json::from_value(params) {
                        Ok(p) => p,
                        Err(e) => {
                            return vec![OutgoingMessage::error(
                                id,
                                -32602,
                                format!("Invalid params: {}", e),
                            )];
                        }
                    };
                handlers::rename::handle_prepare_rename(self, id, params).await
            }
            lsp_types::request::SignatureHelpRequest::METHOD => {
                let params: lsp_types::SignatureHelpParams = match serde_json::from_value(params) {
                    Ok(p) => p,
                    Err(e) => {
                        return vec![OutgoingMessage::error(
                            id,
                            -32602,
                            format!("Invalid params: {}", e),
                        )];
                    }
                };
                handlers::signature_help::handle_signature_help(self, id, params).await
            }
            lsp_types::request::Formatting::METHOD => {
                let params: lsp_types::DocumentFormattingParams =
                    match serde_json::from_value(params) {
                        Ok(p) => p,
                        Err(e) => {
                            return vec![OutgoingMessage::error(
                                id,
                                -32602,
                                format!("Invalid params: {}", e),
                            )];
                        }
                    };
                handlers::formatting::handle_formatting(self, id, params).await
            }
            lsp_types::request::SemanticTokensFullRequest::METHOD => {
                let params: lsp_types::SemanticTokensParams = match serde_json::from_value(params) {
                    Ok(p) => p,
                    Err(e) => {
                        return vec![OutgoingMessage::error(
                            id,
                            -32602,
                            format!("Invalid params: {}", e),
                        )];
                    }
                };
                handlers::semantic_tokens::handle_semantic_tokens_full(self, id, params).await
            }
            _ => vec![OutgoingMessage::error(
                id,
                -32601,
                format!("Method not found: {}", method),
            )],
        }
    }

    async fn handle_notification(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Vec<OutgoingMessage> {
        match method {
            lsp_types::notification::Initialized::METHOD => {
                self.initialized = true;
                vec![]
            }
            lsp_types::notification::Exit::METHOD => {
                handlers::lifecycle::handle_exit(self);
                vec![]
            }
            lsp_types::notification::DidOpenTextDocument::METHOD => {
                let params: lsp_types::DidOpenTextDocumentParams =
                    match serde_json::from_value(params) {
                        Ok(p) => p,
                        Err(_) => return vec![],
                    };
                handlers::text_sync::handle_did_open(self, params).await
            }
            lsp_types::notification::DidChangeTextDocument::METHOD => {
                let params: lsp_types::DidChangeTextDocumentParams =
                    match serde_json::from_value(params) {
                        Ok(p) => p,
                        Err(_) => return vec![],
                    };
                handlers::text_sync::handle_did_change(self, params).await
            }
            lsp_types::notification::DidCloseTextDocument::METHOD => {
                let params: lsp_types::DidCloseTextDocumentParams =
                    match serde_json::from_value(params) {
                        Ok(p) => p,
                        Err(_) => return vec![],
                    };
                handlers::text_sync::handle_did_close(self, params).await
            }
            lsp_types::notification::DidSaveTextDocument::METHOD => {
                let params: lsp_types::DidSaveTextDocumentParams =
                    match serde_json::from_value(params) {
                        Ok(p) => p,
                        Err(_) => return vec![],
                    };
                handlers::text_sync::handle_did_save(self, params).await
            }
            _ => vec![],
        }
    }

    pub fn should_exit(&self) -> bool {
        self.exited
    }

    pub fn exit_code(&self) -> i32 {
        if self.shutdown_requested { 0 } else { 1 }
    }
}

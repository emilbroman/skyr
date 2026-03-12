use std::collections::HashSet;
use std::path::PathBuf;

use lsp_types as lsp;
use serde::{Deserialize, Serialize};

use crate::analysis::{self, OverlaySource, parse_uri, uri_to_path};
use crate::document::DocumentCache;
use crate::handlers;

/// JSON-RPC request/notification ID.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Number(i64),
    String(String),
}

/// Incoming JSON-RPC message from the client.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum IncomingMessage {
    Request {
        id: RequestId,
        method: String,
        #[serde(default)]
        params: serde_json::Value,
    },
    Notification {
        method: String,
        #[serde(default)]
        params: serde_json::Value,
    },
}

/// Outgoing JSON-RPC message to the client.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum OutgoingMessage {
    Response(ResponseMessage),
    Notification(NotificationMessage),
}

#[derive(Debug, Serialize)]
pub struct ResponseMessage {
    pub jsonrpc: &'static str,
    pub id: RequestId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ResponseError>,
}

#[derive(Debug, Serialize)]
pub struct ResponseError {
    pub code: i32,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct NotificationMessage {
    pub jsonrpc: &'static str,
    pub method: String,
    pub params: serde_json::Value,
}

impl OutgoingMessage {
    pub fn response(id: RequestId, result: serde_json::Value) -> Self {
        OutgoingMessage::Response(ResponseMessage {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        })
    }

    pub fn error(id: RequestId, code: i32, message: String) -> Self {
        OutgoingMessage::Response(ResponseMessage {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(ResponseError { code, message }),
        })
    }

    pub fn notification(method: &str, params: serde_json::Value) -> Self {
        OutgoingMessage::Notification(NotificationMessage {
            jsonrpc: "2.0",
            method: method.to_string(),
            params,
        })
    }
}

type SourceFactory =
    Box<dyn Fn(&sclc::ModuleId, DocumentCache, &PathBuf) -> OverlaySource<FsLike> + Send>;

/// The main LSP server.
pub struct LanguageServer {
    /// Factory for creating source repositories.
    source_factory: SourceFactory,
    /// In-memory document overlay (open files from editor).
    documents: DocumentCache,
    /// Workspace root path.
    root: Option<PathBuf>,
    /// Package ID for the workspace.
    package_id: sclc::ModuleId,
    /// Whether shutdown has been requested.
    shutdown_requested: bool,
    /// URI strings that had diagnostics published (for clearing stale diagnostics).
    published_uris: HashSet<String>,
}

/// A filesystem-based source that the LSP wraps with OverlaySource.
pub struct FsLike {
    pub root: PathBuf,
    pub package_id: sclc::ModuleId,
}

impl sclc::SourceRepo for FsLike {
    type Err = std::io::Error;

    fn package_id(&self) -> sclc::ModuleId {
        self.package_id.clone()
    }

    async fn read_file(&self, path: &std::path::Path) -> Result<Option<Vec<u8>>, Self::Err> {
        let result: Result<Vec<u8>, std::io::Error> = tokio::fs::read(self.root.join(path)).await;
        match result {
            Ok(data) => Ok(Some(data)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err),
        }
    }
}

fn default_source_factory() -> SourceFactory {
    Box::new(|package_id, documents, root| {
        let inner = FsLike {
            root: root.clone(),
            package_id: package_id.clone(),
        };
        OverlaySource::new(inner, documents, root.clone())
    })
}

impl Default for LanguageServer {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageServer {
    pub fn new() -> Self {
        Self {
            source_factory: default_source_factory(),
            documents: DocumentCache::new(),
            root: None,
            package_id: sclc::ModuleId::default(),
            shutdown_requested: false,
            published_uris: HashSet::new(),
        }
    }

    /// Handle an incoming message, returning any outgoing messages.
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
        if self.shutdown_requested && method != "shutdown" {
            return vec![OutgoingMessage::error(
                id,
                -32600,
                "Server is shutting down".to_string(),
            )];
        }

        match method {
            "initialize" => handlers::lifecycle::initialize(id, params),
            "shutdown" => {
                self.shutdown_requested = true;
                handlers::lifecycle::shutdown(id)
            }
            "textDocument/hover" => handlers::hover::hover(id, params, &self.documents),
            "textDocument/definition" => {
                handlers::navigation::goto_definition(id, params, &self.documents)
            }
            "textDocument/references" => {
                handlers::navigation::references(id, params, &self.documents)
            }
            "textDocument/documentSymbol" => {
                handlers::navigation::document_symbol(id, params, &self.documents)
            }
            "textDocument/completion" => {
                handlers::completion::completion(id, params, &self.documents)
            }
            _ => vec![OutgoingMessage::error(
                id,
                -32601,
                format!("Method not found: {method}"),
            )],
        }
    }

    async fn handle_notification(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Vec<OutgoingMessage> {
        match method {
            "initialized" => {
                // Client acknowledged initialization — nothing to do
                vec![]
            }
            "exit" => {
                std::process::exit(if self.shutdown_requested { 0 } else { 1 });
            }
            "textDocument/didOpen" => self.handle_did_open(params).await,
            "textDocument/didChange" => self.handle_did_change(params).await,
            "textDocument/didClose" => self.handle_did_close(params).await,
            "textDocument/didSave" => self.handle_did_save(params).await,
            _ => vec![],
        }
    }

    async fn handle_did_open(&mut self, params: serde_json::Value) -> Vec<OutgoingMessage> {
        let params: lsp::DidOpenTextDocumentParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(_) => return vec![],
        };

        self.init_root_from_uri(&params.text_document.uri);

        let path = match uri_to_path(&params.text_document.uri) {
            Some(p) => p,
            None => return vec![],
        };

        self.documents.open(
            path,
            params.text_document.text,
            params.text_document.version,
        );

        self.publish_diagnostics().await
    }

    async fn handle_did_change(&mut self, params: serde_json::Value) -> Vec<OutgoingMessage> {
        let params: lsp::DidChangeTextDocumentParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(_) => return vec![],
        };

        let path = match uri_to_path(&params.text_document.uri) {
            Some(p) => p,
            None => return vec![],
        };

        // We request full sync, so the last content change has the full text
        if let Some(change) = params.content_changes.into_iter().last() {
            self.documents
                .update(&path, change.text, params.text_document.version);
        }

        self.publish_diagnostics().await
    }

    async fn handle_did_close(&mut self, params: serde_json::Value) -> Vec<OutgoingMessage> {
        let params: lsp::DidCloseTextDocumentParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(_) => return vec![],
        };

        let path = match uri_to_path(&params.text_document.uri) {
            Some(p) => p,
            None => return vec![],
        };

        self.documents.close(&path);

        // Clear diagnostics for the closed document
        let mut result = vec![OutgoingMessage::notification(
            "textDocument/publishDiagnostics",
            serde_json::to_value(lsp::PublishDiagnosticsParams {
                uri: params.text_document.uri,
                diagnostics: vec![],
                version: None,
            })
            .unwrap(),
        )];

        // Re-publish diagnostics for remaining documents
        result.extend(self.publish_diagnostics().await);
        result
    }

    async fn handle_did_save(&mut self, _params: serde_json::Value) -> Vec<OutgoingMessage> {
        // Re-run analysis on save
        self.publish_diagnostics().await
    }

    fn init_root_from_uri(&mut self, uri: &lsp::Uri) {
        if self.root.is_some() {
            return;
        }
        if let Some(path) = uri_to_path(uri)
            && let Some(parent) = path.parent()
        {
            self.root = Some(parent.to_path_buf());
            // Derive package ID from directory name
            if let Some(name) = parent.file_name() {
                self.package_id = sclc::ModuleId::from([name.to_string_lossy().as_ref()]);
            }
        }
    }

    async fn publish_diagnostics(&mut self) -> Vec<OutgoingMessage> {
        let root = match &self.root {
            Some(r) => r.clone(),
            None => return vec![],
        };

        let source = (self.source_factory)(&self.package_id, self.documents.clone(), &root);
        let result = analysis::analyze(source, &root).await;

        let mut messages = Vec::new();

        // Clear diagnostics for URIs that no longer have any
        let new_uris: HashSet<String> = result.diagnostics.keys().cloned().collect();
        for old_uri in &self.published_uris {
            if !new_uris.contains(old_uri) {
                messages.push(OutgoingMessage::notification(
                    "textDocument/publishDiagnostics",
                    serde_json::to_value(lsp::PublishDiagnosticsParams {
                        uri: parse_uri(old_uri),
                        diagnostics: vec![],
                        version: None,
                    })
                    .unwrap(),
                ));
            }
        }

        // Publish current diagnostics
        for (uri_str, diagnostics) in &result.diagnostics {
            let uri = parse_uri(uri_str);
            let version = uri_to_path(&uri).and_then(|p| self.documents.version(&p));
            messages.push(OutgoingMessage::notification(
                "textDocument/publishDiagnostics",
                serde_json::to_value(lsp::PublishDiagnosticsParams {
                    uri,
                    diagnostics: diagnostics.clone(),
                    version,
                })
                .unwrap(),
            ));
        }

        self.published_uris = new_uris;
        messages
    }
}

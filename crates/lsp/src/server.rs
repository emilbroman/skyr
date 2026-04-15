use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use lsp_types as lsp;
use serde::{Deserialize, Serialize};

use crate::analysis::{self, OverlayPackage, parse_uri, uri_to_path};
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

impl<'de> Deserialize<'de> for IncomingMessage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let mut obj: serde_json::Map<String, serde_json::Value> =
            serde_json::Map::deserialize(deserializer)?;

        let method = obj
            .remove("method")
            .and_then(|v| v.as_str().map(String::from))
            .ok_or_else(|| serde::de::Error::missing_field("method"))?;

        let params = obj.remove("params").unwrap_or(serde_json::Value::Null);

        if let Some(id_value) = obj.remove("id") {
            let id: RequestId =
                serde_json::from_value(id_value).map_err(serde::de::Error::custom)?;
            Ok(IncomingMessage::Request { id, method, params })
        } else {
            Ok(IncomingMessage::Notification { method, params })
        }
    }
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

/// Serialize a value to JSON, logging and returning Null on failure.
fn to_json_value(value: &impl Serialize) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or_else(|err| {
        eprintln!("lsp: failed to serialize value: {err}");
        serde_json::Value::Null
    })
}

/// The concrete program type used in the LSP server.
pub type LspProgram = sclc::Asg;

/// The main LSP server.
pub struct LanguageServer {
    /// In-memory document overlay (open files from editor).
    documents: DocumentCache,
    /// Workspace root path.
    root: Option<PathBuf>,
    /// Package ID for the workspace.
    package_id: sclc::PackageId,
    /// Whether shutdown has been requested.
    shutdown_requested: bool,
    /// Exit code set when the "exit" notification is received.
    /// The caller should check `exit_code()` after each `handle()` call.
    exit_code: Option<i32>,
    /// URI strings that had diagnostics published (for clearing stale diagnostics).
    published_uris: HashSet<String>,
    /// Extra package finders for cached cross-repo dependencies.
    cached_dep_finders: Vec<Arc<dyn sclc::PackageFinder>>,
    /// Whether we've attempted to resolve cached deps.
    deps_resolved: bool,
    /// Per-package root directories for file URI resolution. Populated by the
    /// CLI from resolved dependency cache paths (including Std).
    package_roots: HashMap<sclc::PackageId, PathBuf>,
}

impl Default for LanguageServer {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageServer {
    pub fn new() -> Self {
        Self {
            documents: DocumentCache::new(),
            root: None,
            package_id: sclc::PackageId::from(["Local"]),
            shutdown_requested: false,
            exit_code: None,
            published_uris: HashSet::new(),
            cached_dep_finders: Vec::new(),
            deps_resolved: false,
            package_roots: HashMap::new(),
        }
    }

    /// Set pre-resolved dependency finders (e.g. from a prior network
    /// fetch) so that cross-repo imports resolve immediately.
    ///
    /// When finders are set this way, the lazy disk-based discovery in
    /// [`publish_diagnostics`] is skipped.
    pub fn set_cached_dep_finders(&mut self, finders: Vec<Arc<dyn sclc::PackageFinder>>) {
        self.cached_dep_finders = finders;
        self.deps_resolved = true;
    }

    /// Set per-package root directories for file URI resolution.
    ///
    /// The CLI populates this from resolved dependency cache paths so that
    /// cross-package navigation (go-to-definition, find-references) can
    /// construct file URIs pointing to the correct location on disk.
    pub fn set_package_roots(&mut self, roots: HashMap<sclc::PackageId, PathBuf>) {
        self.package_roots = roots;
    }

    /// Returns the workspace root path, if known.
    pub fn root(&self) -> Option<&PathBuf> {
        self.root.as_ref()
    }

    /// Returns the package ID for the workspace.
    pub fn package_id(&self) -> &sclc::PackageId {
        &self.package_id
    }

    /// Re-publish diagnostics for all open documents.
    ///
    /// Useful after changing the dependency finders so that cross-repo
    /// import errors are resolved immediately.
    pub async fn refresh_diagnostics(&mut self) -> Vec<OutgoingMessage> {
        self.publish_diagnostics().await
    }

    /// Returns the exit code if the server received an "exit" notification.
    ///
    /// The caller should check this after each `handle()` call and terminate
    /// the event loop when `Some` is returned.
    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code
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

    fn build_finder(&self) -> Option<Arc<dyn sclc::PackageFinder>> {
        let root = self.root.as_ref()?;
        let fs_pkg = sclc::FsPackage::new(root.clone(), self.package_id.clone());
        let overlay = OverlayPackage::new(fs_pkg, self.documents.clone(), root.clone());

        if self.cached_dep_finders.is_empty() {
            Some(sclc::build_default_finder(Arc::new(overlay)))
        } else {
            let std_pkg = Arc::new(sclc::StdPackage::new());
            let mut finders: Vec<Arc<dyn sclc::PackageFinder>> = Vec::new();
            finders.push(sclc::wrap_as_finder(Arc::new(overlay)));
            finders.extend(self.cached_dep_finders.iter().cloned());
            finders.push(sclc::wrap_as_finder(std_pkg));
            Some(Arc::new(sclc::CompositePackageFinder::new(finders)))
        }
    }

    async fn load_program(&self) -> Option<LspProgram> {
        let finder = self.build_finder()?;
        let root = self.root.as_ref()?;
        let extra_paths = self.documents.paths();
        analysis::load_workspace_asg(finder, root, &self.package_id, &extra_paths).await
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
            "textDocument/hover" => {
                let program = self.load_program().await;
                handlers::hover::hover(
                    id,
                    params,
                    &self.documents,
                    program.as_ref(),
                    self.root.as_deref(),
                    &self.package_id,
                )
            }
            "textDocument/definition" => {
                let program = self.load_program().await;
                handlers::navigation::goto_definition(
                    id,
                    params,
                    &self.documents,
                    program.as_ref(),
                    self.root.as_deref(),
                    &self.package_id,
                    &self.package_roots,
                )
            }
            "textDocument/references" => {
                let program = self.load_program().await;
                handlers::navigation::references(
                    id,
                    params,
                    &self.documents,
                    program.as_ref(),
                    self.root.as_deref(),
                    &self.package_id,
                    &self.package_roots,
                )
            }
            "textDocument/prepareRename" => {
                let program = self.load_program().await;
                handlers::navigation::prepare_rename(
                    id,
                    params,
                    &self.documents,
                    program.as_ref(),
                    self.root.as_deref(),
                    &self.package_id,
                )
            }
            "textDocument/rename" => {
                let program = self.load_program().await;
                handlers::navigation::rename(
                    id,
                    params,
                    &self.documents,
                    program.as_ref(),
                    self.root.as_deref(),
                    &self.package_id,
                    &self.package_roots,
                )
            }
            "textDocument/documentSymbol" => handlers::navigation::document_symbol(
                id,
                params,
                &self.documents,
                self.root.as_deref(),
                &self.package_id,
            ),
            "textDocument/completion" => {
                let program = self.load_program().await;
                handlers::completion::completion(
                    id,
                    params,
                    &self.documents,
                    program.as_ref(),
                    self.root.as_deref(),
                    &self.package_id,
                )
            }
            "textDocument/formatting" => {
                handlers::formatting::formatting(id, params, &self.documents)
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
                self.exit_code = Some(if self.shutdown_requested { 0 } else { 1 });
                vec![]
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
            to_json_value(&lsp::PublishDiagnosticsParams {
                uri: params.text_document.uri,
                diagnostics: vec![],
                version: None,
            }),
        )];

        // Re-publish diagnostics for remaining documents
        result.extend(self.publish_diagnostics().await);
        result
    }

    async fn handle_did_save(&mut self, _params: serde_json::Value) -> Vec<OutgoingMessage> {
        // Re-run analysis on save
        self.publish_diagnostics().await
    }

    /// Try to discover cached cross-repo dependencies from disk.
    ///
    /// Reads the workspace's `Package.scle` manifest and looks for
    /// already-cached package versions under `~/.cache/skyr-packages/`.
    /// If any are found, they are added as extra finders so cross-repo
    /// imports resolve. This does not fetch anything from the network;
    /// the cache must have been populated by a prior `skyr run` or
    /// `skyr repl` invocation, or by the CLI `lsp` subcommand at
    /// startup.
    async fn try_resolve_cached_deps(&mut self) {
        let Some(root) = &self.root else { return };
        let root = root.clone();

        let fs_pkg: Arc<dyn sclc::Package> =
            Arc::new(sclc::FsPackage::new(root, self.package_id.clone()));
        let finder = sclc::build_default_finder(Arc::clone(&fs_pkg));

        let manifest = match sclc::load_manifest(fs_pkg, finder).await {
            Ok(Some(m)) => m,
            _ => return,
        };

        let home = match std::env::var("HOME") {
            Ok(h) => PathBuf::from(h),
            Err(_) => return,
        };
        let cache_root = home.join(".cache").join("skyr-packages");

        for repo_qid in manifest.dependencies.keys() {
            let pkg_id =
                sclc::PackageId::from([repo_qid.org.to_string(), repo_qid.repo.to_string()]);
            let pkg_dir_name = pkg_id.to_string().replace('/', "-");
            let pkg_cache_dir = cache_root.join(&pkg_dir_name);

            // Use the first cached version found (most recently cached by
            // a prior CLI run).
            let Ok(mut entries) = tokio::fs::read_dir(&pkg_cache_dir).await else {
                continue;
            };
            if let Ok(Some(entry)) = entries.next_entry().await {
                let version_dir = entry.path();
                if version_dir.is_dir() {
                    let dep_pkg = sclc::FsPackage::new(version_dir, pkg_id);
                    self.cached_dep_finders
                        .push(sclc::wrap_as_finder(Arc::new(dep_pkg)));
                }
            }
        }
    }

    fn init_root_from_uri(&mut self, uri: &lsp::Uri) {
        if self.root.is_some() {
            return;
        }
        if let Some(path) = uri_to_path(uri)
            && let Some(parent) = path.parent()
        {
            // Canonicalize the path to resolve symlinks and ".." components.
            let canonical = parent
                .canonicalize()
                .unwrap_or_else(|_| parent.to_path_buf());
            self.root = Some(canonical);
        }
    }

    async fn publish_diagnostics(&mut self) -> Vec<OutgoingMessage> {
        let root = match &self.root {
            Some(r) => r.clone(),
            None => return vec![],
        };

        // Lazily resolve cached dependencies on first diagnostics pass.
        if !self.deps_resolved {
            self.deps_resolved = true;
            self.try_resolve_cached_deps().await;
        }

        let finder = match self.build_finder() {
            Some(f) => f,
            None => return vec![],
        };
        let extra_paths = self.documents.paths();
        let mut result =
            analysis::analyze_workspace(finder, &root, &self.package_id, &extra_paths).await;

        // Analyse any open .scle file that lives outside the discovered
        // workspace tree (e.g. a scratch file the user opened from elsewhere).
        for path in self.documents.paths() {
            if analysis::is_scle_path(&path) && !result.analyzed_paths.contains(&path) {
                let scle_finder = match self.build_finder() {
                    Some(f) => f,
                    None => continue,
                };
                let diagnostics =
                    analysis::analyze_scle(scle_finder, &path, &root, &self.package_id).await;
                let uri = format!("file://{}", path.display());
                result.diagnostics.insert(uri, diagnostics);
            }
        }

        let mut messages = Vec::new();

        // Clear diagnostics for URIs that no longer have any
        let new_uris: HashSet<String> = result.diagnostics.keys().cloned().collect();
        for old_uri in &self.published_uris {
            if !new_uris.contains(old_uri) {
                messages.push(OutgoingMessage::notification(
                    "textDocument/publishDiagnostics",
                    to_json_value(&lsp::PublishDiagnosticsParams {
                        uri: parse_uri(old_uri),
                        diagnostics: vec![],
                        version: None,
                    }),
                ));
            }
        }

        // Publish current diagnostics
        for (uri_str, diagnostics) in &result.diagnostics {
            let uri = parse_uri(uri_str);
            let version = uri_to_path(&uri).and_then(|p| self.documents.version(&p));
            messages.push(OutgoingMessage::notification(
                "textDocument/publishDiagnostics",
                to_json_value(&lsp::PublishDiagnosticsParams {
                    uri,
                    diagnostics: diagnostics.clone(),
                    version,
                }),
            ));
        }

        self.published_uris = new_uris;
        messages
    }
}

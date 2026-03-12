use lsp_types::{
    InitializeParams, InitializeResult, ServerCapabilities, ServerInfo, TextDocumentSyncCapability,
    TextDocumentSyncKind,
};
use sclc::SourceRepo;

use crate::{LanguageServer, convert::uri_to_path};

#[allow(deprecated)] // Using root_uri for simplicity; workspace_folders support can be added later
pub fn handle_initialize<S: SourceRepo>(
    server: &mut LanguageServer<S>,
    params: InitializeParams,
) -> InitializeResult {
    if let Some(root_uri) = params.root_uri {
        server.root_path = uri_to_path(&root_uri);
    }

    InitializeResult {
        capabilities: ServerCapabilities {
            text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
            ..Default::default()
        },
        server_info: Some(ServerInfo {
            name: "scl-language-server".to_string(),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        }),
    }
}

pub fn handle_shutdown<S: SourceRepo>(server: &mut LanguageServer<S>) {
    server.shutdown_requested = true;
}

pub fn handle_exit<S: SourceRepo>(server: &mut LanguageServer<S>) {
    server.shutdown_requested = true;
}

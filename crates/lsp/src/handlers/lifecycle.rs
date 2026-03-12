use lsp_types::{
    CompletionOptions, HoverProviderCapability, InitializeParams, InitializeResult, OneOf,
    ServerCapabilities, ServerInfo, TextDocumentSyncCapability, TextDocumentSyncKind,
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
            document_symbol_provider: Some(OneOf::Left(true)),
            definition_provider: Some(OneOf::Left(true)),
            hover_provider: Some(HoverProviderCapability::Simple(true)),
            references_provider: Some(OneOf::Left(true)),
            completion_provider: Some(CompletionOptions {
                resolve_provider: Some(true),
                trigger_characters: Some(vec![".".to_string()]),
                ..Default::default()
            }),
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

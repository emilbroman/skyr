use lsp_types::{
    HoverProviderCapability, InitializeParams, InitializeResult, OneOf, RenameOptions,
    ServerCapabilities, ServerInfo, SignatureHelpOptions, TextDocumentSyncCapability,
    TextDocumentSyncKind,
};
use sclc::SourceRepo;

use crate::handlers::semantic_tokens::semantic_tokens_capability;
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
            rename_provider: Some(OneOf::Right(RenameOptions {
                prepare_provider: Some(true),
                work_done_progress_options: Default::default(),
            })),
            signature_help_provider: Some(SignatureHelpOptions {
                trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
                retrigger_characters: None,
                work_done_progress_options: Default::default(),
            }),
            semantic_tokens_provider: Some(semantic_tokens_capability()),
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
    server.exited = true;
}

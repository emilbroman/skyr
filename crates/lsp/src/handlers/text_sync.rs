use std::collections::HashMap;
use std::path::PathBuf;

use lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, PublishDiagnosticsParams, Uri,
};
use sclc::SourceRepo;

use crate::convert::{diag_to_lsp, path_to_uri, uri_to_path};
use crate::overlay::OverlaySource;
use crate::{LanguageServer, OutgoingMessage};

pub async fn handle_did_open<S: SourceRepo + 'static>(
    server: &mut LanguageServer<S>,
    params: DidOpenTextDocumentParams,
) -> Vec<OutgoingMessage> {
    let uri = params.text_document.uri;
    let content = params.text_document.text;
    let version = params.text_document.version;

    {
        let mut documents = server.documents.lock().await;
        documents.open(&uri, content, version);
    }

    compile_and_publish(server, &uri).await
}

pub async fn handle_did_change<S: SourceRepo + 'static>(
    server: &mut LanguageServer<S>,
    params: DidChangeTextDocumentParams,
) -> Vec<OutgoingMessage> {
    let uri = params.text_document.uri;
    let version = params.text_document.version;

    if let Some(change) = params.content_changes.into_iter().last() {
        let mut documents = server.documents.lock().await;
        documents.update(&uri, change.text, version);
    }

    compile_and_publish(server, &uri).await
}

pub async fn handle_did_close<S: SourceRepo + 'static>(
    server: &mut LanguageServer<S>,
    params: DidCloseTextDocumentParams,
) -> Vec<OutgoingMessage> {
    let uri = params.text_document.uri;

    {
        let mut documents = server.documents.lock().await;
        documents.close(&uri);
    }

    vec![OutgoingMessage::notification(
        "textDocument/publishDiagnostics",
        PublishDiagnosticsParams {
            uri,
            diagnostics: vec![],
            version: None,
        },
    )]
}

pub async fn handle_did_save<S: SourceRepo + 'static>(
    server: &mut LanguageServer<S>,
    params: DidSaveTextDocumentParams,
) -> Vec<OutgoingMessage> {
    compile_and_publish(server, &params.text_document.uri).await
}

async fn compile_and_publish<S: SourceRepo + 'static>(
    server: &mut LanguageServer<S>,
    changed_uri: &Uri,
) -> Vec<OutgoingMessage> {
    let Some(root_path) = server.root_path.clone() else {
        return vec![];
    };

    let changed_path = uri_to_path(changed_uri);
    if let Some(ref path) = changed_path
        && path.extension().is_none_or(|ext| ext != "scl")
    {
        return vec![];
    }

    let source = (server.source_factory)();
    let overlay = OverlaySource::new(source, server.documents.clone(), root_path.clone());

    let result = sclc::compile(overlay).await;

    let diagnosed = match result {
        Ok(d) => d,
        Err(_) => {
            return vec![];
        }
    };

    let mut diagnostics_by_path: HashMap<PathBuf, Vec<lsp_types::Diagnostic>> = HashMap::new();

    for diag in diagnosed.diags().iter() {
        let (module_id, _span) = diag.locate();
        let file_path = module_id_to_path(&root_path, &module_id);

        diagnostics_by_path
            .entry(file_path)
            .or_default()
            .push(diag_to_lsp(diag));
    }

    let mut messages = Vec::new();

    if let Some(changed_path) = changed_path {
        let diagnostics = diagnostics_by_path
            .remove(&changed_path)
            .unwrap_or_default();
        if let Some(uri) = path_to_uri(&changed_path) {
            messages.push(OutgoingMessage::notification(
                "textDocument/publishDiagnostics",
                PublishDiagnosticsParams {
                    uri,
                    diagnostics,
                    version: None,
                },
            ));
        }
    }

    for (path, diagnostics) in diagnostics_by_path {
        if let Some(uri) = path_to_uri(&path) {
            messages.push(OutgoingMessage::notification(
                "textDocument/publishDiagnostics",
                PublishDiagnosticsParams {
                    uri,
                    diagnostics,
                    version: None,
                },
            ));
        }
    }

    messages
}

fn module_id_to_path(root_path: &std::path::Path, module_id: &sclc::ModuleId) -> PathBuf {
    let segments = module_id.as_slice();
    if segments.len() < 3 {
        return root_path.to_path_buf();
    }

    let file_segments = &segments[2..];
    let mut path = root_path.to_path_buf();
    for (i, segment) in file_segments.iter().enumerate() {
        if i == file_segments.len() - 1 {
            path.push(format!("{}.scl", segment));
        } else {
            path.push(segment);
        }
    }
    path
}

// TODO: Add completion support for .scle files (see hover.rs TODO).
use std::path::Path;

use lsp_types as lsp;

use super::{lock_cursor_info, resolve_document};
use crate::analysis;
use crate::convert;
use crate::document::DocumentCache;
use crate::server::{LspProgram, OutgoingMessage, RequestId, to_json_value};

pub fn completion(
    id: RequestId,
    params: serde_json::Value,
    documents: &DocumentCache,
    program: Option<&LspProgram>,
    root: Option<&Path>,
    package_id: &sclc::PackageId,
) -> Vec<OutgoingMessage> {
    let params: lsp::CompletionParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(_) => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let ctx = match resolve_document(
        &params.text_document_position.text_document.uri,
        documents,
        root,
        package_id,
    ) {
        Some(ctx) => ctx,
        None => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let position = convert::to_sclc_position(params.text_document_position.position);
    let cursor_info = match program {
        Some(program) => analysis::query_cursor(program, &ctx.source, &ctx.module_id, position),
        None => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let info = lock_cursor_info(&cursor_info);

    if info.completion_candidates.is_empty() {
        return vec![OutgoingMessage::response(id, serde_json::Value::Null)];
    }

    let items: Vec<lsp::CompletionItem> = info
        .completion_candidates
        .iter()
        .map(|candidate| match candidate {
            sclc::CompletionCandidate::Var(name) => lsp::CompletionItem {
                label: name.clone(),
                kind: Some(lsp::CompletionItemKind::VARIABLE),
                ..Default::default()
            },
            sclc::CompletionCandidate::Member(member) => lsp::CompletionItem {
                label: member.name.clone(),
                kind: Some(lsp::CompletionItemKind::FIELD),
                detail: member
                    .ty
                    .as_ref()
                    .map(|ty| format!("let {}: {ty}", member.name)),
                documentation: member
                    .description
                    .as_ref()
                    .map(|desc| lsp::Documentation::String(desc.clone())),
                ..Default::default()
            },
            sclc::CompletionCandidate::Module(name) => lsp::CompletionItem {
                label: name.clone(),
                kind: Some(lsp::CompletionItemKind::MODULE),
                ..Default::default()
            },
            sclc::CompletionCandidate::ModuleDir(name) => lsp::CompletionItem {
                label: name.clone(),
                kind: Some(lsp::CompletionItemKind::FOLDER),
                ..Default::default()
            },
            sclc::CompletionCandidate::PathFile(name) => lsp::CompletionItem {
                label: name.clone(),
                kind: Some(lsp::CompletionItemKind::FILE),
                ..Default::default()
            },
            sclc::CompletionCandidate::PathDir(name) => lsp::CompletionItem {
                label: name.clone(),
                kind: Some(lsp::CompletionItemKind::FOLDER),
                ..Default::default()
            },
        })
        .collect();

    let result = to_json_value(&items);
    vec![OutgoingMessage::response(id, result)]
}

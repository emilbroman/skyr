use lsp_types as lsp;

use crate::analysis::{self, uri_to_path};
use crate::convert;
use crate::document::DocumentCache;
use crate::server::{OutgoingMessage, RequestId};

pub fn completion(
    id: RequestId,
    params: serde_json::Value,
    documents: &DocumentCache,
) -> Vec<OutgoingMessage> {
    let params: lsp::CompletionParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(_) => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let path = match uri_to_path(&params.text_document_position.text_document.uri) {
        Some(p) => p,
        None => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let source = match documents.get(&path) {
        Some(s) => s,
        None => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let module_id = module_id_from_path(&path);
    let position = convert::to_sclc_position(params.text_document_position.position);
    let cursor_info = analysis::query_cursor(&source, &module_id, position);

    let info = cursor_info.lock().unwrap();

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
            sclc::CompletionCandidate::Member(name) => lsp::CompletionItem {
                label: name.clone(),
                kind: Some(lsp::CompletionItemKind::FIELD),
                ..Default::default()
            },
        })
        .collect();

    let result = serde_json::to_value(items).unwrap();
    vec![OutgoingMessage::response(id, result)]
}

fn module_id_from_path(path: &std::path::Path) -> sclc::ModuleId {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let parent_name = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Local".to_string());
    sclc::ModuleId::from([parent_name, stem])
}

use lsp_types as lsp;

use crate::analysis::uri_to_path;
use crate::document::DocumentCache;
use crate::server::{OutgoingMessage, RequestId};

pub fn formatting(
    id: RequestId,
    params: serde_json::Value,
    documents: &DocumentCache,
) -> Vec<OutgoingMessage> {
    let params: lsp::DocumentFormattingParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(_) => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let path = match uri_to_path(&params.text_document.uri) {
        Some(p) => p,
        None => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let source = match documents.get(&path) {
        Some(s) => s,
        None => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let module_id = module_id_from_path(&path);
    let diagnosed = sclc::parse_file_mod(&source, &module_id);

    // If there are parse errors, don't format — return null so the editor
    // keeps the current text and the user sees the diagnostics instead.
    if diagnosed.diags().has_errors() {
        return vec![OutgoingMessage::response(id, serde_json::Value::Null)];
    }

    let file_mod = diagnosed.into_inner();

    let formatted = sclc::Formatter::format(&source, &file_mod);

    // If the formatted output is the same, return an empty edit list
    if formatted == source {
        return vec![OutgoingMessage::response(
            id,
            serde_json::to_value(Vec::<lsp::TextEdit>::new()).unwrap(),
        )];
    }

    // Replace the entire document with the formatted text
    let line_count = source.lines().count().max(1) as u32;
    let last_line_len = source.lines().last().map(|l| l.len() as u32).unwrap_or(0);

    let edit = lsp::TextEdit {
        range: lsp::Range {
            start: lsp::Position {
                line: 0,
                character: 0,
            },
            end: lsp::Position {
                line: line_count,
                character: last_line_len,
            },
        },
        new_text: formatted,
    };

    vec![OutgoingMessage::response(
        id,
        serde_json::to_value(vec![edit]).unwrap(),
    )]
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

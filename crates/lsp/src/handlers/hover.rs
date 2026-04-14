use std::path::Path;

use lsp_types as lsp;

// TODO: Add hover support for .scle files. The synthesised __Scle__/Main ASG
// produced by evaluate_scle could support hover, but cursor positions in the
// original SCLE source need to map correctly into the synthesised module.
use super::{lock_cursor_info, resolve_document};
use crate::analysis;
use crate::convert;
use crate::document::DocumentCache;
use crate::server::{LspProgram, OutgoingMessage, RequestId};

pub fn hover(
    id: RequestId,
    params: serde_json::Value,
    documents: &DocumentCache,
    program: Option<&LspProgram>,
    root: Option<&Path>,
    package_id: &sclc::PackageId,
) -> Vec<OutgoingMessage> {
    let params: lsp::HoverParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(_) => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let ctx = match resolve_document(
        &params.text_document_position_params.text_document.uri,
        documents,
        root,
        package_id,
    ) {
        Some(ctx) => ctx,
        None => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let position = convert::to_sclc_position(params.text_document_position_params.position);
    let cursor_info = match program {
        Some(program) => analysis::query_cursor(program, &ctx.source, &ctx.module_id, position),
        None => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let info = lock_cursor_info(&cursor_info);
    let mut parts = Vec::new();
    let type_value = match (&info.identifier, &info.ty) {
        (Some(sclc::CursorIdentifier::Let(name)), Some(ty)) => Some(format!("let {name}: {ty}")),
        (Some(sclc::CursorIdentifier::Type(name)), Some(ty)) => Some(format!("type {name} {ty}")),
        (None, Some(ty)) => Some(ty.to_string()),
        _ => None,
    };
    if let Some(value) = type_value {
        parts.push(lsp::MarkedString::LanguageString(lsp::LanguageString {
            language: "scl".to_string(),
            value,
        }));
    }
    if let Some(description) = &info.description {
        parts.push(lsp::MarkedString::String(description.clone()));
    }
    let result = if parts.is_empty() {
        serde_json::Value::Null
    } else {
        let hover = lsp::Hover {
            contents: lsp::HoverContents::Array(parts),
            range: None,
        };
        serde_json::to_value(hover).unwrap_or_else(|err| {
            eprintln!("lsp: failed to serialize hover result: {err}");
            serde_json::Value::Null
        })
    };

    vec![OutgoingMessage::response(id, result)]
}

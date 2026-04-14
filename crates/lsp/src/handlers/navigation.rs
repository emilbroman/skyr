// TODO: Add goto-definition/references/document-symbol support for .scle files
// (see hover.rs TODO).
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use lsp_types as lsp;

use super::{lock_cursor_info, resolve_document};
use crate::analysis::{self, raw_module_id_to_uri};
use crate::convert;
use crate::document::DocumentCache;
use crate::server::{LspProgram, OutgoingMessage, RequestId};

pub fn goto_definition(
    id: RequestId,
    params: serde_json::Value,
    documents: &DocumentCache,
    program: Option<&LspProgram>,
    root: Option<&Path>,
    package_id: &sclc::PackageId,
    package_roots: &HashMap<sclc::PackageId, PathBuf>,
) -> Vec<OutgoingMessage> {
    let params: lsp::GotoDefinitionParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(_) => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let uri = params
        .text_document_position_params
        .text_document
        .uri
        .clone();
    let ctx = match resolve_document(&uri, documents, root, package_id) {
        Some(ctx) => ctx,
        None => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let asg = match program {
        Some(asg) => asg,
        None => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let position = convert::to_sclc_position(params.text_document_position_params.position);
    let cursor_info = analysis::query_cursor(asg, &ctx.source, &ctx.module_id, position);

    let info = lock_cursor_info(&cursor_info);
    let result = match &info.declaration {
        Some((decl_module_id, decl_span)) => {
            let decl_uri = raw_module_id_to_uri(asg, decl_module_id, root, package_roots)
                .unwrap_or_else(|| uri.clone());
            let location = lsp::Location {
                uri: decl_uri,
                range: convert::to_lsp_range(*decl_span),
            };
            serde_json::to_value(location).unwrap_or_else(|err| {
                eprintln!("lsp: failed to serialize definition result: {err}");
                serde_json::Value::Null
            })
        }
        None => serde_json::Value::Null,
    };

    vec![OutgoingMessage::response(id, result)]
}

pub fn references(
    id: RequestId,
    params: serde_json::Value,
    documents: &DocumentCache,
    program: Option<&LspProgram>,
    root: Option<&Path>,
    package_id: &sclc::PackageId,
    package_roots: &HashMap<sclc::PackageId, PathBuf>,
) -> Vec<OutgoingMessage> {
    let params: lsp::ReferenceParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(_) => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let uri = params.text_document_position.text_document.uri.clone();
    let ctx = match resolve_document(&uri, documents, root, package_id) {
        Some(ctx) => ctx,
        None => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let asg = match program {
        Some(asg) => asg,
        None => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let position = convert::to_sclc_position(params.text_document_position.position);
    let cursor_info = analysis::query_cursor(asg, &ctx.source, &ctx.module_id, position);

    let info = lock_cursor_info(&cursor_info);

    let mut locations: Vec<lsp::Location> = info
        .references
        .iter()
        .filter_map(|(ref_module_id, span)| {
            let ref_uri = raw_module_id_to_uri(asg, ref_module_id, root, package_roots)?;
            Some(lsp::Location {
                uri: ref_uri,
                range: convert::to_lsp_range(*span),
            })
        })
        .collect();

    // Include the declaration itself if requested
    if params.context.include_declaration
        && let Some((decl_module_id, decl_span)) = &info.declaration
    {
        let decl_uri = raw_module_id_to_uri(asg, decl_module_id, root, package_roots)
            .unwrap_or_else(|| uri.clone());
        locations.insert(
            0,
            lsp::Location {
                uri: decl_uri,
                range: convert::to_lsp_range(*decl_span),
            },
        );
    }

    let result = if locations.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::to_value(locations).unwrap_or_else(|err| {
            eprintln!("lsp: failed to serialize references result: {err}");
            serde_json::Value::Null
        })
    };

    vec![OutgoingMessage::response(id, result)]
}

pub fn document_symbol(
    id: RequestId,
    params: serde_json::Value,
    documents: &DocumentCache,
    root: Option<&Path>,
    package_id: &sclc::PackageId,
) -> Vec<OutgoingMessage> {
    let params: lsp::DocumentSymbolParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(_) => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let ctx = match resolve_document(&params.text_document.uri, documents, root, package_id) {
        Some(ctx) => ctx,
        None => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let symbols = analysis::document_symbols(&ctx.source, &ctx.module_id);

    let result = serde_json::to_value(symbols).unwrap_or_else(|err| {
        eprintln!("lsp: failed to serialize document symbols: {err}");
        serde_json::Value::Null
    });
    vec![OutgoingMessage::response(id, result)]
}

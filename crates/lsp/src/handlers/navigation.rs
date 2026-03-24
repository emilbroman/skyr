use lsp_types as lsp;

use crate::analysis::{self, module_id_from_path, uri_to_path};
use crate::convert;
use crate::document::DocumentCache;
use crate::server::{LspProgram, OutgoingMessage, RequestId};

pub fn goto_definition(
    id: RequestId,
    params: serde_json::Value,
    documents: &DocumentCache,
    program: Option<&LspProgram>,
) -> Vec<OutgoingMessage> {
    let params: lsp::GotoDefinitionParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(_) => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let uri = &params.text_document_position_params.text_document.uri;
    let path = match uri_to_path(uri) {
        Some(p) => p,
        None => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let source = match documents.get(&path) {
        Some(s) => s,
        None => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let module_id = module_id_from_path(&path);
    let position = convert::to_sclc_position(params.text_document_position_params.position);
    let cursor_info = match program {
        Some(program) => analysis::query_cursor(program, &source, &module_id, position),
        None => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let info = cursor_info.lock().unwrap();
    let result = match info.declaration {
        Some(decl_span) => {
            let location = lsp::Location {
                uri: uri.clone(),
                range: convert::to_lsp_range(decl_span),
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
) -> Vec<OutgoingMessage> {
    let params: lsp::ReferenceParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(_) => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let uri = &params.text_document_position.text_document.uri;
    let path = match uri_to_path(uri) {
        Some(p) => p,
        None => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let source = match documents.get(&path) {
        Some(s) => s,
        None => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let module_id = module_id_from_path(&path);
    let position = convert::to_sclc_position(params.text_document_position.position);
    let cursor_info = match program {
        Some(program) => analysis::query_cursor(program, &source, &module_id, position),
        None => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let info = cursor_info.lock().unwrap();

    let mut locations: Vec<lsp::Location> = info
        .references
        .iter()
        .map(|span| lsp::Location {
            uri: uri.clone(),
            range: convert::to_lsp_range(*span),
        })
        .collect();

    // Include the declaration itself if requested
    if params.context.include_declaration
        && let Some(decl_span) = info.declaration
    {
        locations.insert(
            0,
            lsp::Location {
                uri: uri.clone(),
                range: convert::to_lsp_range(decl_span),
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
) -> Vec<OutgoingMessage> {
    let params: lsp::DocumentSymbolParams = match serde_json::from_value(params) {
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
    let symbols = analysis::document_symbols(&source, &module_id);

    let result = serde_json::to_value(symbols).unwrap_or_else(|err| {
        eprintln!("lsp: failed to serialize document symbols: {err}");
        serde_json::Value::Null
    });
    vec![OutgoingMessage::response(id, result)]
}

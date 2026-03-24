use lsp_types as lsp;

use crate::analysis::{self, module_id_from_path, uri_to_path};
use crate::convert;
use crate::document::DocumentCache;
use crate::server::{LspProgram, OutgoingMessage, RequestId};

pub fn hover(
    id: RequestId,
    params: serde_json::Value,
    documents: &DocumentCache,
    program: Option<&LspProgram>,
) -> Vec<OutgoingMessage> {
    let params: lsp::HoverParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(_) => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let path = match uri_to_path(&params.text_document_position_params.text_document.uri) {
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
    let result = match &info.ty {
        Some(ty) => {
            let hover = lsp::Hover {
                contents: lsp::HoverContents::Scalar(lsp::MarkedString::String(ty.to_string())),
                range: None,
            };
            serde_json::to_value(hover).unwrap_or_else(|err| {
                eprintln!("lsp: failed to serialize hover result: {err}");
                serde_json::Value::Null
            })
        }
        None => serde_json::Value::Null,
    };

    vec![OutgoingMessage::response(id, result)]
}

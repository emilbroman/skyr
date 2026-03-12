use std::collections::HashMap;

use lsp_types::{
    PrepareRenameResponse, RenameParams, TextDocumentPositionParams, TextEdit, WorkspaceEdit,
};
use sclc::SourceRepo;

use crate::convert::{lsp_to_position, path_to_uri, span_to_range, uri_to_path};
use crate::helpers::{find_module_by_path, module_id_to_path};
use crate::query::{self, NodeKind};
use crate::{LanguageServer, OutgoingMessage, RequestId};

pub async fn handle_rename<S: SourceRepo + 'static>(
    server: &LanguageServer<S>,
    id: RequestId,
    params: RenameParams,
) -> Vec<OutgoingMessage> {
    let uri = &params.text_document_position.text_document.uri;
    let lsp_pos = params.text_document_position.position;
    let pos = lsp_to_position(lsp_pos);
    let new_name = &params.new_name;

    let Some(path) = uri_to_path(uri) else {
        return vec![OutgoingMessage::response(id, Option::<WorkspaceEdit>::None)];
    };

    let Some(program) = server.last_program.as_ref() else {
        return vec![OutgoingMessage::response(id, Option::<WorkspaceEdit>::None)];
    };

    let Some((module_id, file_mod)) = find_module_by_path(program, &server.root_path, &path) else {
        return vec![OutgoingMessage::response(id, Option::<WorkspaceEdit>::None)];
    };

    let Some(node) = query::node_at_position(file_mod, pos) else {
        return vec![OutgoingMessage::response(id, Option::<WorkspaceEdit>::None)];
    };

    let var_name = match node.kind {
        NodeKind::Var(var) => &var.name,
        NodeKind::LetBindVar(bind) => &bind.var.name,
        NodeKind::Property { .. } => {
            return vec![OutgoingMessage::response(id, Option::<WorkspaceEdit>::None)];
        }
    };

    let root = server
        .root_path
        .as_deref()
        .unwrap_or(std::path::Path::new("."));

    // Collect references in the current file.
    let spans = query::find_var_references(file_mod, var_name);
    let def_path = module_id_to_path(root, &module_id);

    let Some(file_uri) = path_to_uri(&def_path) else {
        return vec![OutgoingMessage::response(id, Option::<WorkspaceEdit>::None)];
    };

    let edits: Vec<TextEdit> = spans
        .into_iter()
        .map(|span| TextEdit {
            range: span_to_range(span),
            new_text: new_name.clone(),
        })
        .collect();

    #[allow(clippy::mutable_key_type)] // lsp_types::Uri has interior mutability
    let mut changes = HashMap::new();
    changes.insert(file_uri, edits);

    vec![OutgoingMessage::response(
        id,
        Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
    )]
}

pub async fn handle_prepare_rename<S: SourceRepo + 'static>(
    server: &LanguageServer<S>,
    id: RequestId,
    params: TextDocumentPositionParams,
) -> Vec<OutgoingMessage> {
    let uri = &params.text_document.uri;
    let lsp_pos = params.position;
    let pos = lsp_to_position(lsp_pos);

    let Some(path) = uri_to_path(uri) else {
        return vec![OutgoingMessage::response(
            id,
            Option::<PrepareRenameResponse>::None,
        )];
    };

    let Some(program) = server.last_program.as_ref() else {
        return vec![OutgoingMessage::response(
            id,
            Option::<PrepareRenameResponse>::None,
        )];
    };

    let Some((_module_id, file_mod)) = find_module_by_path(program, &server.root_path, &path)
    else {
        return vec![OutgoingMessage::response(
            id,
            Option::<PrepareRenameResponse>::None,
        )];
    };

    let Some(node) = query::node_at_position(file_mod, pos) else {
        return vec![OutgoingMessage::response(
            id,
            Option::<PrepareRenameResponse>::None,
        )];
    };

    let result = match node.kind {
        NodeKind::Var(var) => Some(PrepareRenameResponse::RangeWithPlaceholder {
            range: span_to_range(var.span()),
            placeholder: var.name.clone(),
        }),
        NodeKind::LetBindVar(bind) => Some(PrepareRenameResponse::RangeWithPlaceholder {
            range: span_to_range(bind.var.span()),
            placeholder: bind.var.name.clone(),
        }),
        NodeKind::Property { .. } => None,
    };

    vec![OutgoingMessage::response(id, result)]
}

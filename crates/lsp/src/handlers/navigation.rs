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

/// Returns `true` if `name` is a valid SCL identifier (matches the lexer's
/// rules in [`crate::lexer`]) and is not a reserved keyword.
fn is_valid_scl_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_alphabetic() || first == '_') {
        return false;
    }
    if !chars.all(|c| c.is_alphanumeric() || c == '_') {
        return false;
    }
    !matches!(
        name,
        "import"
            | "let"
            | "fn"
            | "export"
            | "extern"
            | "if"
            | "else"
            | "for"
            | "in"
            | "nil"
            | "true"
            | "false"
            | "exception"
            | "raise"
            | "try"
            | "catch"
            | "type"
            | "as"
    )
}

/// Returns `true` if the declaration's module belongs to the workspace
/// package (so it is safe to rename — we won't be modifying a dependency or
/// the standard library).
fn is_local_declaration(
    asg: &sclc::Asg,
    decl_module_id: &sclc::RawModuleId,
    workspace_package: &sclc::PackageId,
) -> bool {
    asg.module(decl_module_id)
        .is_some_and(|m| m.package_id == *workspace_package)
}

fn span_contains_position(span: sclc::Span, position: sclc::Position) -> bool {
    position >= span.start() && position <= span.end()
}

/// Find the span at `position` inside the cursor's module by checking the
/// declaration and every reference recorded in `info`.
fn span_at_cursor(
    info: &sclc::CursorInfo,
    cursor_module: &sclc::RawModuleId,
    position: sclc::Position,
) -> Option<sclc::Span> {
    if let Some((decl_module, decl_span)) = &info.declaration
        && decl_module == cursor_module
        && span_contains_position(*decl_span, position)
    {
        return Some(*decl_span);
    }
    info.references
        .iter()
        .find(|(ref_module, span)| {
            ref_module == cursor_module && span_contains_position(*span, position)
        })
        .map(|(_, span)| *span)
}

pub fn prepare_rename(
    id: RequestId,
    params: serde_json::Value,
    documents: &DocumentCache,
    program: Option<&LspProgram>,
    root: Option<&Path>,
    package_id: &sclc::PackageId,
) -> Vec<OutgoingMessage> {
    let params: lsp::TextDocumentPositionParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(_) => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let uri = params.text_document.uri.clone();
    let ctx = match resolve_document(&uri, documents, root, package_id) {
        Some(ctx) => ctx,
        None => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let asg = match program {
        Some(asg) => asg,
        None => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let position = convert::to_sclc_position(params.position);
    let cursor_info = analysis::query_cursor(asg, &ctx.source, &ctx.module_id, position);
    let info = lock_cursor_info(&cursor_info);

    let Some((decl_module_id, _)) = &info.declaration else {
        return vec![OutgoingMessage::response(id, serde_json::Value::Null)];
    };

    if !is_local_declaration(asg, decl_module_id, package_id) {
        return vec![OutgoingMessage::response(id, serde_json::Value::Null)];
    }

    let cursor_module: sclc::RawModuleId = ctx.module_id.all_segments();
    let result = match span_at_cursor(&info, &cursor_module, position) {
        Some(span) => {
            let response = lsp::PrepareRenameResponse::Range(convert::to_lsp_range(span));
            serde_json::to_value(response).unwrap_or_else(|err| {
                eprintln!("lsp: failed to serialize prepare_rename: {err}");
                serde_json::Value::Null
            })
        }
        None => serde_json::Value::Null,
    };
    vec![OutgoingMessage::response(id, result)]
}

// `lsp::Uri` has interior mutability, so it triggers `clippy::mutable_key_type`
// when used as a `HashMap` key. We need it as a key for the `WorkspaceEdit`
// `changes` map which is what the LSP protocol requires.
#[allow(clippy::mutable_key_type)]
pub fn rename(
    id: RequestId,
    params: serde_json::Value,
    documents: &DocumentCache,
    program: Option<&LspProgram>,
    root: Option<&Path>,
    package_id: &sclc::PackageId,
    package_roots: &HashMap<sclc::PackageId, PathBuf>,
) -> Vec<OutgoingMessage> {
    let params: lsp::RenameParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(err) => {
            return vec![OutgoingMessage::error(
                id,
                -32602,
                format!("Invalid rename params: {err}"),
            )];
        }
    };

    if !is_valid_scl_identifier(&params.new_name) {
        return vec![OutgoingMessage::error(
            id,
            -32602,
            format!("'{}' is not a valid identifier", params.new_name),
        )];
    }

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

    let Some((decl_module_id, decl_span)) = &info.declaration else {
        return vec![OutgoingMessage::response(id, serde_json::Value::Null)];
    };

    if !is_local_declaration(asg, decl_module_id, package_id) {
        return vec![OutgoingMessage::error(
            id,
            -32803,
            "Cannot rename declarations outside the workspace package".to_string(),
        )];
    }

    // Group edits by URI string first to avoid clippy::mutable_key_type
    // (lsp::Uri has interior mutability).
    let mut edits_by_uri: HashMap<String, (lsp::Uri, Vec<lsp::TextEdit>)> = HashMap::new();

    if let Some(decl_uri) = raw_module_id_to_uri(asg, decl_module_id, root, package_roots) {
        edits_by_uri
            .entry(decl_uri.as_str().to_owned())
            .or_insert_with(|| (decl_uri.clone(), Vec::new()))
            .1
            .push(lsp::TextEdit {
                range: convert::to_lsp_range(*decl_span),
                new_text: params.new_name.clone(),
            });
    }

    for (ref_module_id, ref_span) in &info.references {
        if let Some(ref_uri) = raw_module_id_to_uri(asg, ref_module_id, root, package_roots) {
            edits_by_uri
                .entry(ref_uri.as_str().to_owned())
                .or_insert_with(|| (ref_uri.clone(), Vec::new()))
                .1
                .push(lsp::TextEdit {
                    range: convert::to_lsp_range(*ref_span),
                    new_text: params.new_name.clone(),
                });
        }
    }

    let changes: HashMap<lsp::Uri, Vec<lsp::TextEdit>> = edits_by_uri.into_values().collect();

    let edit = lsp::WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    };
    let result = serde_json::to_value(edit).unwrap_or_else(|err| {
        eprintln!("lsp: failed to serialize rename result: {err}");
        serde_json::Value::Null
    });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_identifier_accepts_simple_names() {
        assert!(is_valid_scl_identifier("foo"));
        assert!(is_valid_scl_identifier("Foo"));
        assert!(is_valid_scl_identifier("_foo"));
        assert!(is_valid_scl_identifier("foo_bar"));
        assert!(is_valid_scl_identifier("foo123"));
        assert!(is_valid_scl_identifier("café"));
    }

    #[test]
    fn valid_identifier_rejects_empty_or_invalid_starts() {
        assert!(!is_valid_scl_identifier(""));
        assert!(!is_valid_scl_identifier("1foo"));
        assert!(!is_valid_scl_identifier(" foo"));
        assert!(!is_valid_scl_identifier("foo-bar"));
        assert!(!is_valid_scl_identifier("foo bar"));
        assert!(!is_valid_scl_identifier("foo.bar"));
    }

    #[test]
    fn valid_identifier_rejects_keywords() {
        for kw in [
            "import",
            "let",
            "fn",
            "export",
            "extern",
            "if",
            "else",
            "for",
            "in",
            "nil",
            "true",
            "false",
            "exception",
            "raise",
            "try",
            "catch",
            "type",
            "as",
        ] {
            assert!(!is_valid_scl_identifier(kw), "{kw} should be rejected");
        }
    }

    fn span(line_start: u32, col_start: u32, line_end: u32, col_end: u32) -> sclc::Span {
        sclc::Span::new(
            sclc::Position::new(line_start, col_start),
            sclc::Position::new(line_end, col_end),
        )
    }

    #[test]
    fn span_contains_position_handles_inclusive_bounds() {
        let s = span(1, 5, 1, 10);
        assert!(span_contains_position(s, sclc::Position::new(1, 5)));
        assert!(span_contains_position(s, sclc::Position::new(1, 7)));
        assert!(span_contains_position(s, sclc::Position::new(1, 10)));
        assert!(!span_contains_position(s, sclc::Position::new(1, 4)));
        assert!(!span_contains_position(s, sclc::Position::new(1, 11)));
    }
}

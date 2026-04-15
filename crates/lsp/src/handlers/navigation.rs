// TODO: Add goto-definition/references/document-symbol support for .scle files
// (see hover.rs TODO).
use std::collections::{HashMap, HashSet};
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
    let cursor_info =
        analysis::query_cursor_with_references(asg, &ctx.source, &ctx.module_id, position);

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
    let cursor_info =
        analysis::query_cursor_with_references(asg, &ctx.source, &ctx.module_id, position);
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

/// Collect the spans of record-shorthand fields (`{ x }` rather than
/// `{ x: x }`) whose name matches `name` within the given module body.
///
/// A field is shorthand iff its `var.span()` and the inner `Var` expression's
/// span are identical — the parser produces this exact shape for the
/// shorthand sugar.
struct ShorthandCollector<'a> {
    name: &'a str,
    spans: HashSet<sclc::Span>,
}

impl sclc::Visitor for ShorthandCollector<'_> {
    fn visit_path(&mut self, _path: &sclc::PathExpr, _span: sclc::Span) {}

    fn visit_record_field(&mut self, field: &sclc::RecordField) {
        if field.var.name != self.name {
            return;
        }
        if let sclc::Expr::Var(inner) = field.expr.as_ref()
            && inner.name == self.name
            && field.expr.span() == field.var.span()
        {
            self.spans.insert(field.var.span());
        }
    }
}

fn collect_shorthand_spans(body: &sclc::ModuleBody, name: &str) -> HashSet<sclc::Span> {
    let mut collector = ShorthandCollector {
        name,
        spans: HashSet::new(),
    };
    walk_module_body(body, &mut collector);
    collector.spans
}

fn walk_module_body(body: &sclc::ModuleBody, visitor: &mut dyn sclc::Visitor) {
    match body {
        sclc::ModuleBody::File(file_mod) => sclc::visit_file_mod(visitor, file_mod),
        sclc::ModuleBody::Scle(scle_mod) => sclc::visit_scle_mod(visitor, scle_mod),
    }
}

/// Collects spans of every record-field declaration in a module — both
/// `RecordTypeFieldExpr.var.span()` (in type expressions) and
/// `RecordField.var.span()` (in record literals). Used by the rename handler
/// to determine whether a declaration site is a field declaration vs. a
/// regular variable binding.
#[derive(Default)]
struct FieldDeclSpansCollector {
    spans: HashSet<sclc::Span>,
}

impl sclc::Visitor for FieldDeclSpansCollector {
    fn visit_path(&mut self, _path: &sclc::PathExpr, _span: sclc::Span) {}

    fn visit_record_field(&mut self, field: &sclc::RecordField) {
        self.spans.insert(field.var.span());
    }

    fn visit_record_type_field(&mut self, field: &sclc::RecordTypeFieldExpr) {
        self.spans.insert(field.var.span());
    }
}

fn collect_field_decl_spans(body: &sclc::ModuleBody) -> HashSet<sclc::Span> {
    let mut collector = FieldDeclSpansCollector::default();
    walk_module_body(body, &mut collector);
    collector.spans
}

/// Returns the original identifier name that the cursor is on, if any.
fn cursor_identifier_name(info: &sclc::CursorInfo) -> Option<&str> {
    match info.identifier.as_ref()? {
        sclc::CursorIdentifier::Let(name) | sclc::CursorIdentifier::Type(name) => {
            Some(name.as_str())
        }
    }
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
    let cursor_info =
        analysis::query_cursor_with_references(asg, &ctx.source, &ctx.module_id, position);
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

    // Determine whether we're renaming a record-field declaration (vs. a
    // regular variable / type binding). The shorthand-expansion rule
    // depends on which side of `field: var` is changing.
    let is_field_rename = asg
        .module(decl_module_id)
        .map(|m| collect_field_decl_spans(&m.body).contains(decl_span))
        .unwrap_or(false);

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

    // For each module that contains references, collect the spans of any
    // record-shorthand fields that match the renamed identifier so we can
    // expand `{ x }` into `{ x: y }` instead of producing `{ y }` (which
    // would silently rename the field too).
    let original_name = cursor_identifier_name(&info);
    let mut shorthand_spans_by_module: HashMap<sclc::RawModuleId, HashSet<sclc::Span>> =
        HashMap::new();
    if let Some(name) = original_name {
        let mut modules: HashSet<&sclc::RawModuleId> = HashSet::new();
        for (ref_module_id, _) in &info.references {
            modules.insert(ref_module_id);
        }
        for raw_id in modules {
            if let Some(module) = asg.module(raw_id) {
                let spans = collect_shorthand_spans(&module.body, name);
                if !spans.is_empty() {
                    shorthand_spans_by_module.insert(raw_id.clone(), spans);
                }
            }
        }
    }

    for (ref_module_id, ref_span) in &info.references {
        if let Some(ref_uri) = raw_module_id_to_uri(asg, ref_module_id, root, package_roots) {
            let is_shorthand = shorthand_spans_by_module
                .get(ref_module_id)
                .is_some_and(|s| s.contains(ref_span));
            let new_text = if is_shorthand {
                // Expand shorthand so the field-name and value-var halves
                // stay independent. We know `original_name` is `Some`
                // because `is_shorthand` was set.
                let original = original_name.unwrap_or(&params.new_name);
                if is_field_rename {
                    // Renaming the field: `{ x }` → `{ y: x }`.
                    format!("{}: {}", params.new_name, original)
                } else {
                    // Renaming the variable: `{ x }` → `{ x: y }`.
                    format!("{}: {}", original, params.new_name)
                }
            } else {
                params.new_name.clone()
            };
            edits_by_uri
                .entry(ref_uri.as_str().to_owned())
                .or_insert_with(|| (ref_uri.clone(), Vec::new()))
                .1
                .push(lsp::TextEdit {
                    range: convert::to_lsp_range(*ref_span),
                    new_text,
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

    fn parse_body(source: &str) -> sclc::ModuleBody {
        let module_id = sclc::ModuleId::default();
        let file_mod = sclc::parse_file_mod(source, &module_id).into_inner();
        sclc::ModuleBody::File(file_mod)
    }

    #[test]
    fn shorthand_collector_finds_shorthand_field() {
        // `let x = 1\n{ x }` — `x` appears once as shorthand.
        let body = parse_body("let x = 1\n{ x }\n");
        let spans = collect_shorthand_spans(&body, "x");
        assert_eq!(spans.len(), 1);
    }

    #[test]
    fn shorthand_collector_skips_explicit_field() {
        // `{ x: x }` is not shorthand — the field name and the value have
        // different spans.
        let body = parse_body("let x = 1\n{ x: x }\n");
        let spans = collect_shorthand_spans(&body, "x");
        assert!(spans.is_empty());
    }

    #[test]
    fn shorthand_collector_ignores_other_names() {
        let body = parse_body("let x = 1\nlet y = 2\n{ y }\n");
        let spans = collect_shorthand_spans(&body, "x");
        assert!(spans.is_empty());
    }

    #[test]
    fn shorthand_collector_handles_nested_records() {
        let body = parse_body("let x = 1\n{ outer: { x } }\n");
        let spans = collect_shorthand_spans(&body, "x");
        assert_eq!(spans.len(), 1);
    }

    #[test]
    fn field_decl_collector_finds_record_literal_fields() {
        let body = parse_body("let r = { x: 1, y: 2 }\n");
        let spans = collect_field_decl_spans(&body);
        assert_eq!(spans.len(), 2);
    }

    #[test]
    fn field_decl_collector_finds_record_type_fields() {
        let body = parse_body("let r: { x: Int, y: Str } = { x: 1, y: \"hi\" }\n");
        let spans = collect_field_decl_spans(&body);
        // Two type fields + two literal fields.
        assert_eq!(spans.len(), 4);
    }

    #[test]
    fn field_decl_collector_finds_type_def_fields() {
        let body = parse_body("type R = { x: Int, y: Str }\n");
        let spans = collect_field_decl_spans(&body);
        assert_eq!(spans.len(), 2);
    }

    #[test]
    fn field_decl_collector_finds_fn_param_record_fields() {
        let body = parse_body("let f = fn (r: { x: Int }) -> r.x\n");
        let spans = collect_field_decl_spans(&body);
        assert_eq!(spans.len(), 1);
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

use lsp_types as lsp;

use crate::analysis::is_scle_path;
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

    // Formatting only needs path + source, not a full module ID for the
    // workspace package, so we use a lightweight resolve here.
    let ctx = match super::resolve_document(
        &params.text_document.uri,
        documents,
        None,
        &sclc::PackageId::from(["Local"]),
    ) {
        Some(ctx) => ctx,
        None => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    let formatted = if is_scle_path(&ctx.path) {
        format_scle(&ctx.source)
    } else {
        format_scl(&ctx.source, &ctx.module_id)
    };

    let formatted = match formatted {
        Some(f) => f,
        None => return vec![OutgoingMessage::response(id, serde_json::Value::Null)],
    };

    // If the formatted output is the same, return an empty edit list
    if formatted == ctx.source {
        let empty_edits = serde_json::to_value(Vec::<lsp::TextEdit>::new()).unwrap_or_else(|err| {
            eprintln!("lsp: failed to serialize empty edits: {err}");
            serde_json::Value::Null
        });
        return vec![OutgoingMessage::response(id, empty_edits)];
    }

    // Replace the entire document with the formatted text
    let line_count = ctx.source.lines().count().max(1) as u32;
    let last_line_len = ctx
        .source
        .lines()
        .last()
        .map(|l| l.len() as u32)
        .unwrap_or(0);

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

    let result = serde_json::to_value(vec![edit]).unwrap_or_else(|err| {
        eprintln!("lsp: failed to serialize formatting edits: {err}");
        serde_json::Value::Null
    });
    vec![OutgoingMessage::response(id, result)]
}

/// Format an `.scl` file. Returns `None` if parsing fails.
fn format_scl(source: &str, module_id: &sclc::ModuleId) -> Option<String> {
    let diagnosed = sclc::parse_file_mod(source, module_id);

    // If there are parse errors, don't format.
    if diagnosed.diags().has_errors() {
        return None;
    }

    let file_mod = diagnosed.into_inner();
    Some(sclc::Formatter::format(source, &file_mod))
}

/// Format an `.scle` file. Returns `None` if parsing fails.
fn format_scle(source: &str) -> Option<String> {
    let module_id = sclc::ModuleId::default();
    let diagnosed = sclc::parse_scle(source, &module_id);

    // If there are parse errors, don't format.
    if diagnosed.diags().has_errors() {
        return None;
    }

    let scle_mod = diagnosed.into_inner()?;
    Some(sclc::Formatter::format_scle(source, &scle_mod))
}

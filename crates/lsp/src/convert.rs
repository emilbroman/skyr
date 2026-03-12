use std::path::PathBuf;

use lsp_types::Uri;
use sclc::{Diag, DiagLevel, Position, Span};

/// Convert an sclc Position (1-based) to an LSP Position (0-based).
pub fn position_to_lsp(pos: Position) -> lsp_types::Position {
    lsp_types::Position {
        line: pos.line().saturating_sub(1),
        character: pos.character().saturating_sub(1),
    }
}

/// Convert an sclc Span to an LSP Range.
pub fn span_to_range(span: Span) -> lsp_types::Range {
    lsp_types::Range {
        start: position_to_lsp(span.start()),
        end: position_to_lsp(span.end()),
    }
}

/// Convert an sclc DiagLevel to an LSP DiagnosticSeverity.
pub fn level_to_severity(level: DiagLevel) -> lsp_types::DiagnosticSeverity {
    match level {
        DiagLevel::Error => lsp_types::DiagnosticSeverity::ERROR,
        DiagLevel::Warning => lsp_types::DiagnosticSeverity::WARNING,
    }
}

/// Convert an sclc diagnostic to an LSP Diagnostic.
pub fn diag_to_lsp(diag: &dyn Diag) -> lsp_types::Diagnostic {
    let (_module_id, span) = diag.locate();

    lsp_types::Diagnostic {
        range: span_to_range(span),
        severity: Some(level_to_severity(diag.level())),
        code: None,
        code_description: None,
        source: Some("scl".to_string()),
        message: diag.to_string(),
        related_information: None,
        tags: None,
        data: None,
    }
}

/// Convert an LSP Uri to a PathBuf.
pub fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    if uri.scheme()?.as_str() != "file" {
        return None;
    }
    let path_str = uri.path().as_str();
    Some(PathBuf::from(path_str))
}

/// Convert a PathBuf to an LSP Uri.
pub fn path_to_uri(path: &std::path::Path) -> Option<Uri> {
    let uri_str = format!("file://{}", path.display());
    uri_str.parse().ok()
}

use lsp_types as lsp;

/// Convert an sclc Position (1-based) to an LSP Position (0-based).
pub fn to_lsp_position(pos: sclc::Position) -> lsp::Position {
    lsp::Position {
        line: pos.line().saturating_sub(1),
        character: pos.character().saturating_sub(1),
    }
}

/// Convert an sclc Span to an LSP Range.
pub fn to_lsp_range(span: sclc::Span) -> lsp::Range {
    lsp::Range {
        start: to_lsp_position(span.start()),
        end: to_lsp_position(span.end()),
    }
}

/// Convert an sclc DiagLevel to an LSP DiagnosticSeverity.
pub fn to_lsp_severity(level: sclc::DiagLevel) -> lsp::DiagnosticSeverity {
    match level {
        sclc::DiagLevel::Error => lsp::DiagnosticSeverity::ERROR,
        sclc::DiagLevel::Warning => lsp::DiagnosticSeverity::WARNING,
    }
}

/// Convert an sclc Diag to an LSP Diagnostic.
pub fn to_lsp_diagnostic(diag: &dyn sclc::Diag) -> (sclc::ModuleId, lsp::Diagnostic) {
    let (module_id, span) = diag.locate();
    let diagnostic = lsp::Diagnostic {
        range: to_lsp_range(span),
        severity: Some(to_lsp_severity(diag.level())),
        source: Some("scl".to_string()),
        message: diag.to_string(),
        ..Default::default()
    };
    (module_id, diagnostic)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_converts_one_based_to_zero_based() {
        let sclc_pos = sclc::Position::new(1, 1);
        let lsp_pos = to_lsp_position(sclc_pos);
        assert_eq!(lsp_pos.line, 0);
        assert_eq!(lsp_pos.character, 0);
    }

    #[test]
    fn span_converts_to_range() {
        let span = sclc::Span::new(sclc::Position::new(3, 5), sclc::Position::new(3, 10));
        let range = to_lsp_range(span);
        assert_eq!(range.start.line, 2);
        assert_eq!(range.start.character, 4);
        assert_eq!(range.end.line, 2);
        assert_eq!(range.end.character, 9);
    }
}

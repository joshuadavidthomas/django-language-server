use djls_semantic::collect_template_diagnostics;
use djls_source::File;
use djls_source::FileKind;
use djls_source::PositionEncoding;
use tower_lsp_server::ls_types;

use crate::ext::DiagnosticExt;

/// Collect all LSP diagnostics for a template file.
///
/// Returns `None` when `file` is not a diagnostics target. For template files,
/// triggers parsing and validation via Salsa-tracked queries (cached across
/// calls), then converts the accumulated errors to LSP types. Diagnostics are
/// filtered and severity-adjusted per `diagnostics_config`.
#[must_use]
pub fn collect_diagnostics(
    db: &dyn djls_semantic::Db,
    file: File,
) -> Option<Vec<ls_types::Diagnostic>> {
    collect_diagnostics_with_encoding(db, file, PositionEncoding::Utf8)
}

/// Collect all LSP diagnostics using the client's negotiated position encoding.
#[must_use]
pub fn collect_diagnostics_with_encoding(
    db: &dyn djls_semantic::Db,
    file: File,
    encoding: PositionEncoding,
) -> Option<Vec<ls_types::Diagnostic>> {
    let Ok(source) = file.try_source(db) else {
        return None;
    };
    if *source.kind() != FileKind::Template {
        return None;
    }

    let mut diagnostics = Vec::new();

    let config = db.diagnostics_config();

    let collected = collect_template_diagnostics(db, file);
    let line_index = file.line_index(db);

    for error in collected.template_errors {
        if let Some(diagnostic) =
            error.to_lsp_diagnostic(source.as_str(), line_index, encoding, &config)
        {
            diagnostics.push(diagnostic);
        }
    }

    for error in collected.validation_errors {
        if let Some(diagnostic) =
            error.to_lsp_diagnostic(source.as_str(), line_index, encoding, &config)
        {
            diagnostics.push(diagnostic);
        }
    }

    Some(diagnostics)
}

#[cfg(test)]
mod tests {
    use djls_conf::DiagnosticSeverity;
    use djls_source::LineIndex;
    use djls_templates::ParseError;
    use djls_templates::TemplateError;

    use super::*;
    use crate::ext::DiagnosticSeverityExt;

    #[test]
    fn template_parse_diagnostics_use_legacy_code_and_structured_range() {
        let source = "Hello {{ value";
        let line_index = LineIndex::from(source);
        let error = TemplateError::from(ParseError::MalformedConstruct {
            position: 6,
            opener: "{{".to_string(),
            closer: "}}".to_string(),
            content: "value".to_string(),
        });

        let diagnostic = error
            .to_lsp_diagnostic(
                source,
                &line_index,
                PositionEncoding::Utf8,
                &djls_conf::DiagnosticsConfig::default(),
            )
            .expect("default diagnostic severity should be enabled");

        assert_eq!(
            diagnostic.code,
            Some(ls_types::NumberOrString::String("T100".to_string()))
        );
        assert_eq!(diagnostic.range.start, ls_types::Position::new(0, 6));
        assert_eq!(diagnostic.range.end, ls_types::Position::new(0, 8));
    }

    #[test]
    fn diagnostic_ranges_follow_position_encoding() {
        let source = "é😀 {{ value";
        let line_index = LineIndex::from(source);
        let error = TemplateError::from(ParseError::MalformedConstruct {
            position: 7,
            opener: "{{".to_string(),
            closer: "}}".to_string(),
            content: "value".to_string(),
        });
        let config = djls_conf::DiagnosticsConfig::default();

        let utf16 = error
            .to_lsp_diagnostic(source, &line_index, PositionEncoding::Utf16, &config)
            .expect("UTF-16 diagnostic should be enabled");
        let utf32 = error
            .to_lsp_diagnostic(source, &line_index, PositionEncoding::Utf32, &config)
            .expect("UTF-32 diagnostic should be enabled");

        assert_eq!(
            utf16.range,
            ls_types::Range::new(ls_types::Position::new(0, 4), ls_types::Position::new(0, 6),)
        );
        assert_eq!(
            utf32.range,
            ls_types::Range::new(ls_types::Position::new(0, 3), ls_types::Position::new(0, 5),)
        );
    }

    #[test]
    fn test_to_lsp_severity() {
        assert_eq!(DiagnosticSeverity::Off.to_lsp_severity(), None);
        assert_eq!(
            DiagnosticSeverity::Error.to_lsp_severity(),
            Some(ls_types::DiagnosticSeverity::ERROR)
        );
        assert_eq!(
            DiagnosticSeverity::Warning.to_lsp_severity(),
            Some(ls_types::DiagnosticSeverity::WARNING)
        );
        assert_eq!(
            DiagnosticSeverity::Info.to_lsp_severity(),
            Some(ls_types::DiagnosticSeverity::INFORMATION)
        );
        assert_eq!(
            DiagnosticSeverity::Hint.to_lsp_severity(),
            Some(ls_types::DiagnosticSeverity::HINT)
        );
    }
}

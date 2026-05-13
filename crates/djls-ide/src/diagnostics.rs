use djls_semantic::ValidationError;
use djls_source::File;
use djls_source::LineIndex;
use djls_source::Span;
use djls_templates::TemplateError;
use djls_templates::TemplateErrorAccumulator;
use tower_lsp_server::ls_types;

use crate::ext::DiagnosticSeverityExt;
use crate::ext::SpanExt;

trait DiagnosticError: std::fmt::Display {
    fn span(&self) -> Option<(u32, u32)>;
    fn diagnostic_code(&self) -> &'static str;

    fn message(&self) -> String {
        self.to_string()
    }

    fn as_diagnostic(&self, line_index: &LineIndex) -> ls_types::Diagnostic {
        let range = self
            .span()
            .map(|(start, length)| Span::new(start, length).to_lsp_range(line_index))
            .unwrap_or_default();

        ls_types::Diagnostic {
            range,
            severity: Some(ls_types::DiagnosticSeverity::ERROR),
            code: Some(ls_types::NumberOrString::String(
                self.diagnostic_code().to_string(),
            )),
            code_description: None,
            source: Some(crate::SOURCE_NAME.to_string()),
            message: self.message(),
            related_information: None,
            tags: None,
            data: None,
        }
    }
}

impl DiagnosticError for TemplateError {
    fn span(&self) -> Option<(u32, u32)> {
        self.primary_span()
    }

    fn diagnostic_code(&self) -> &'static str {
        // Calls the inherent method on TemplateError (not recursive —
        // inherent methods take priority over trait methods in resolution).
        TemplateError::diagnostic_code(self)
    }
}

impl DiagnosticError for ValidationError {
    fn span(&self) -> Option<(u32, u32)> {
        self.primary_span().map(Into::into)
    }

    fn diagnostic_code(&self) -> &'static str {
        self.code()
    }
}

fn push_with_severity(
    mut diagnostic: ls_types::Diagnostic,
    config: &djls_conf::diagnostics::DiagnosticsConfig,
    diagnostics: &mut Vec<ls_types::Diagnostic>,
) {
    if let Some(ls_types::NumberOrString::String(code)) = &diagnostic.code {
        let severity = config.get_severity(code);

        if let Some(lsp_severity) = severity.to_lsp_severity() {
            diagnostic.severity = Some(lsp_severity);
            diagnostics.push(diagnostic);
        }
    } else {
        diagnostics.push(diagnostic);
    }
}

/// Collect all LSP diagnostics for a template file.
///
/// Triggers parsing and validation via Salsa-tracked queries (cached
/// across calls), then converts the accumulated errors to LSP types.
/// Diagnostics are filtered and severity-adjusted per `diagnostics_config`.
#[must_use]
pub fn collect_diagnostics(db: &dyn djls_semantic::Db, file: File) -> Vec<ls_types::Diagnostic> {
    let mut diagnostics = Vec::new();

    let config = db.diagnostics_config();

    djls_semantic::validate_template_file(db, file);

    let template_errors =
        djls_templates::parse_template::accumulated::<TemplateErrorAccumulator>(db, file);

    let line_index = file.line_index(db);

    for error_acc in template_errors {
        let diagnostic = error_acc.0.as_diagnostic(line_index);
        push_with_severity(diagnostic, &config, &mut diagnostics);
    }

    let validation_errors = djls_semantic::validate_template_file::accumulated::<
        djls_semantic::ValidationErrorAccumulator,
    >(db, file);

    for error_acc in validation_errors {
        let diagnostic = error_acc.0.as_diagnostic(line_index);
        push_with_severity(diagnostic, &config, &mut diagnostics);
    }

    diagnostics
}

#[cfg(test)]
mod tests {
    use djls_conf::DiagnosticSeverity;
    use djls_templates::ParseError;

    use super::*;

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

        let diagnostic = error.as_diagnostic(&line_index);

        assert_eq!(
            diagnostic.code,
            Some(ls_types::NumberOrString::String("T100".to_string()))
        );
        assert_eq!(diagnostic.range.start, ls_types::Position::new(0, 6));
        assert_eq!(diagnostic.range.end, ls_types::Position::new(0, 8));
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

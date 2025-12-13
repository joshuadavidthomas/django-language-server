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
        None
    }

    fn diagnostic_code(&self) -> &'static str {
        match self {
            TemplateError::Parser(_) => "T100",
            TemplateError::Io(_) => "T900",
            TemplateError::Config(_) => "T901",
        }
    }
}

impl DiagnosticError for ValidationError {
    fn span(&self) -> Option<(u32, u32)> {
        match self {
            ValidationError::UnbalancedStructure { opening_span, .. } => Some(opening_span.into()),
            ValidationError::UnclosedTag { span, .. }
            | ValidationError::OrphanedTag { span, .. }
            | ValidationError::UnmatchedBlockName { span, .. }
            | ValidationError::MissingRequiredArguments { span, .. }
            | ValidationError::TooManyArguments { span, .. }
            | ValidationError::MissingArgument { span, .. }
            | ValidationError::InvalidLiteralArgument { span, .. }
            | ValidationError::InvalidArgumentChoice { span, .. } => Some(span.into()),
        }
    }

    fn diagnostic_code(&self) -> &'static str {
        match self {
            ValidationError::UnclosedTag { .. } => "S100",
            ValidationError::UnbalancedStructure { .. } => "S101",
            ValidationError::OrphanedTag { .. } => "S102",
            ValidationError::UnmatchedBlockName { .. } => "S103",
            ValidationError::MissingRequiredArguments { .. }
            | ValidationError::MissingArgument { .. } => "S104",
            ValidationError::TooManyArguments { .. } => "S105",
            ValidationError::InvalidLiteralArgument { .. } => "S106",
            ValidationError::InvalidArgumentChoice { .. } => "S107",
        }
    }
}

/// Collect all diagnostics for a template file.
///
/// This function collects and converts errors that were accumulated during
/// parsing and validation. The caller must provide the parsed `NodeList` (or `None`
/// if parsing failed), making it explicit that parsing should have already occurred.
///
/// Diagnostics are filtered based on the configuration settings (`select` and `ignore`),
/// and severity levels can be overridden per diagnostic code.
///
/// # Parameters
/// - `db`: The Salsa database
/// - `file`: The source file (needed to retrieve accumulated template errors)
/// - `nodelist`: The parsed AST, or None if parsing failed
///
/// # Returns
/// A vector of LSP diagnostics combining both template syntax errors and
/// semantic validation errors, filtered by the diagnostics configuration.
///
/// # Design
/// This API design makes it clear that:
/// - Parsing must happen before collecting diagnostics
/// - This function only collects and converts existing errors
/// - The `NodeList` provides both line offsets and access to validation errors
#[must_use]
pub fn collect_diagnostics(
    db: &dyn djls_semantic::Db,
    file: File,
    nodelist: Option<djls_templates::NodeList<'_>>,
) -> Vec<ls_types::Diagnostic> {
    let mut diagnostics = Vec::new();

    let config = db.diagnostics_config();

    let template_errors =
        djls_templates::parse_template::accumulated::<TemplateErrorAccumulator>(db, file);

    let line_index = file.line_index(db);

    for error_acc in template_errors {
        let mut diagnostic = error_acc.0.as_diagnostic(line_index);
        if let Some(ls_types::NumberOrString::String(code)) = &diagnostic.code {
            let severity = config.get_severity(code);

            // Skip if diagnostic is disabled (severity = off)
            if let Some(lsp_severity) = severity.to_lsp_severity() {
                diagnostic.severity = Some(lsp_severity);
                diagnostics.push(diagnostic);
            }
        } else {
            // No code, use default
            diagnostics.push(diagnostic);
        }
    }

    if let Some(nodelist) = nodelist {
        let validation_errors = djls_semantic::validate_nodelist::accumulated::<
            djls_semantic::ValidationErrorAccumulator,
        >(db, nodelist);

        for error_acc in validation_errors {
            let mut diagnostic = error_acc.0.as_diagnostic(line_index);
            if let Some(ls_types::NumberOrString::String(code)) = &diagnostic.code {
                let severity = config.get_severity(code);

                // Skip if diagnostic is disabled (severity = off)
                if let Some(lsp_severity) = severity.to_lsp_severity() {
                    diagnostic.severity = Some(lsp_severity);
                    diagnostics.push(diagnostic);
                }
            } else {
                // No code, use default
                diagnostics.push(diagnostic);
            }
        }
    }

    diagnostics
}

#[cfg(test)]
mod tests {
    use djls_conf::DiagnosticSeverity;

    use super::*;

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

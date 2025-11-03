use djls_semantic::ValidationError;
use djls_source::File;
use djls_source::LineIndex;
use djls_source::Span;
use djls_templates::TemplateError;
use djls_templates::TemplateErrorAccumulator;
use tower_lsp_server::lsp_types;

use crate::ext::SpanExt;

trait DiagnosticError: std::fmt::Display {
    fn span(&self) -> Option<(u32, u32)>;
    fn diagnostic_code(&self) -> &'static str;

    fn message(&self) -> String {
        self.to_string()
    }

    fn as_diagnostic(&self, line_index: &LineIndex) -> lsp_types::Diagnostic {
        let range = self
            .span()
            .map(|(start, length)| Span::new(start, length).to_lsp_range(line_index))
            .unwrap_or_default();

        lsp_types::Diagnostic {
            range,
            severity: Some(lsp_types::DiagnosticSeverity::ERROR),
            code: Some(lsp_types::NumberOrString::String(
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
/// Diagnostics can be filtered based on the configuration settings. Any diagnostic
/// codes listed in `disabled_diagnostics` will be excluded from the results.
///
/// # Parameters
/// - `db`: The Salsa database
/// - `file`: The source file (needed to retrieve accumulated template errors)
/// - `nodelist`: The parsed AST, or None if parsing failed
///
/// # Returns
/// A vector of LSP diagnostics combining both template syntax errors and
/// semantic validation errors, filtered by the disabled diagnostics configuration.
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
) -> Vec<lsp_types::Diagnostic> {
    let mut diagnostics = Vec::new();

    let disabled_codes = db.disabled_diagnostics();

    let template_errors =
        djls_templates::parse_template::accumulated::<TemplateErrorAccumulator>(db, file);

    let line_index = file.line_index(db);

    for error_acc in template_errors {
        let diagnostic = error_acc.0.as_diagnostic(line_index);
        if !is_diagnostic_disabled(&diagnostic, &disabled_codes) {
            diagnostics.push(diagnostic);
        }
    }

    if let Some(nodelist) = nodelist {
        let validation_errors = djls_semantic::validate_nodelist::accumulated::<
            djls_semantic::ValidationErrorAccumulator,
        >(db, nodelist);

        for error_acc in validation_errors {
            let diagnostic = error_acc.0.as_diagnostic(line_index);
            if !is_diagnostic_disabled(&diagnostic, &disabled_codes) {
                diagnostics.push(diagnostic);
            }
        }
    }

    diagnostics
}

/// Check if a diagnostic should be filtered out based on the disabled codes.
fn is_diagnostic_disabled(diagnostic: &lsp_types::Diagnostic, disabled_codes: &[String]) -> bool {
    if let Some(lsp_types::NumberOrString::String(code)) = &diagnostic.code {
        disabled_codes.contains(code)
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_diagnostic_disabled_with_matching_code() {
        let diagnostic = lsp_types::Diagnostic {
            range: lsp_types::Range::default(),
            severity: Some(lsp_types::DiagnosticSeverity::ERROR),
            code: Some(lsp_types::NumberOrString::String("S100".to_string())),
            code_description: None,
            source: Some("djls".to_string()),
            message: "Test error".to_string(),
            related_information: None,
            tags: None,
            data: None,
        };

        let disabled_codes = vec!["S100".to_string(), "S101".to_string()];
        assert!(is_diagnostic_disabled(&diagnostic, &disabled_codes));
    }

    #[test]
    fn test_is_diagnostic_disabled_with_non_matching_code() {
        let diagnostic = lsp_types::Diagnostic {
            range: lsp_types::Range::default(),
            severity: Some(lsp_types::DiagnosticSeverity::ERROR),
            code: Some(lsp_types::NumberOrString::String("S102".to_string())),
            code_description: None,
            source: Some("djls".to_string()),
            message: "Test error".to_string(),
            related_information: None,
            tags: None,
            data: None,
        };

        let disabled_codes = vec!["S100".to_string(), "S101".to_string()];
        assert!(!is_diagnostic_disabled(&diagnostic, &disabled_codes));
    }

    #[test]
    fn test_is_diagnostic_disabled_with_empty_disabled_list() {
        let diagnostic = lsp_types::Diagnostic {
            range: lsp_types::Range::default(),
            severity: Some(lsp_types::DiagnosticSeverity::ERROR),
            code: Some(lsp_types::NumberOrString::String("S100".to_string())),
            code_description: None,
            source: Some("djls".to_string()),
            message: "Test error".to_string(),
            related_information: None,
            tags: None,
            data: None,
        };

        let disabled_codes = vec![];
        assert!(!is_diagnostic_disabled(&diagnostic, &disabled_codes));
    }

    #[test]
    fn test_is_diagnostic_disabled_with_no_code() {
        let diagnostic = lsp_types::Diagnostic {
            range: lsp_types::Range::default(),
            severity: Some(lsp_types::DiagnosticSeverity::ERROR),
            code: None,
            code_description: None,
            source: Some("djls".to_string()),
            message: "Test error".to_string(),
            related_information: None,
            tags: None,
            data: None,
        };

        let disabled_codes = vec!["S100".to_string()];
        assert!(!is_diagnostic_disabled(&diagnostic, &disabled_codes));
    }

    #[test]
    fn test_is_diagnostic_disabled_with_numeric_code() {
        let diagnostic = lsp_types::Diagnostic {
            range: lsp_types::Range::default(),
            severity: Some(lsp_types::DiagnosticSeverity::ERROR),
            code: Some(lsp_types::NumberOrString::Number(100)),
            code_description: None,
            source: Some("djls".to_string()),
            message: "Test error".to_string(),
            related_information: None,
            tags: None,
            data: None,
        };

        let disabled_codes = vec!["S100".to_string()];
        assert!(!is_diagnostic_disabled(&diagnostic, &disabled_codes));
    }
}

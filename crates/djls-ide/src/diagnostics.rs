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

// Include the generated lookup table from error enum definitions
// This const array maps error type names to diagnostic codes
include!(concat!(env!("OUT_DIR"), "/diagnostic_codes.rs"));

/// Helper function to look up diagnostic code from the generated mappings
#[inline]
fn lookup_code(error_type: &str) -> &'static str {
    DIAGNOSTIC_CODE_MAPPINGS
        .iter()
        .find(|(type_name, _)| *type_name == error_type)
        .map(|(_, code)| *code)
        .unwrap_or("UNKNOWN")
}

impl DiagnosticError for TemplateError {
    fn span(&self) -> Option<(u32, u32)> {
        None
    }

    fn diagnostic_code(&self) -> &'static str {
        match self {
            TemplateError::Parser(_) => lookup_code("TemplateError::Parser"),
            TemplateError::Io(_) => lookup_code("TemplateError::Io"),
            TemplateError::Config(_) => lookup_code("TemplateError::Config"),
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
            ValidationError::UnclosedTag { .. } => lookup_code("ValidationError::UnclosedTag"),
            ValidationError::UnbalancedStructure { .. } => {
                lookup_code("ValidationError::UnbalancedStructure")
            }
            ValidationError::OrphanedTag { .. } => lookup_code("ValidationError::OrphanedTag"),
            ValidationError::UnmatchedBlockName { .. } => {
                lookup_code("ValidationError::UnmatchedBlockName")
            }
            ValidationError::MissingRequiredArguments { .. } => {
                lookup_code("ValidationError::MissingRequiredArguments")
            }
            ValidationError::TooManyArguments { .. } => {
                lookup_code("ValidationError::TooManyArguments")
            }
            ValidationError::MissingArgument { .. } => {
                lookup_code("ValidationError::MissingArgument")
            }
            ValidationError::InvalidLiteralArgument { .. } => {
                lookup_code("ValidationError::InvalidLiteralArgument")
            }
            ValidationError::InvalidArgumentChoice { .. } => {
                lookup_code("ValidationError::InvalidArgumentChoice")
            }
        }
    }
}

/// Collect all diagnostics for a template file.
///
/// This function collects and converts errors that were accumulated during
/// parsing and validation. The caller must provide the parsed `NodeList` (or `None`
/// if parsing failed), making it explicit that parsing should have already occurred.
///
/// # Parameters
/// - `db`: The Salsa database
/// - `file`: The source file (needed to retrieve accumulated template errors)
/// - `nodelist`: The parsed AST, or None if parsing failed
///
/// # Returns
/// A vector of LSP diagnostics combining both template syntax errors and
/// semantic validation errors.
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

    let template_errors =
        djls_templates::parse_template::accumulated::<TemplateErrorAccumulator>(db, file);

    let line_index = file.line_index(db);

    for error_acc in template_errors {
        diagnostics.push(error_acc.0.as_diagnostic(line_index));
    }

    if let Some(nodelist) = nodelist {
        let validation_errors = djls_semantic::validate_nodelist::accumulated::<
            djls_semantic::ValidationErrorAccumulator,
        >(db, nodelist);

        for error_acc in validation_errors {
            diagnostics.push(error_acc.0.as_diagnostic(line_index));
        }
    }

    diagnostics
}

use djls_semantic::ValidationError;
use djls_source::File;
use djls_source::LineIndex;
use djls_source::Offset;
use djls_source::Span;
use djls_templates::TemplateError;
use djls_templates::TemplateErrorAccumulator;
use tower_lsp_server::lsp_types;

trait DiagnosticError: std::fmt::Display {
    fn span(&self) -> Option<(u32, u32)>;
    fn diagnostic_code(&self) -> &'static str;

    fn message(&self) -> String {
        self.to_string()
    }

    fn as_diagnostic(&self, line_index: &LineIndex) -> lsp_types::Diagnostic {
        let range = self
            .span()
            .map(|(start, length)| {
                let span = Span::new(start, length);
                LspRange::from((&span, line_index)).into()
            })
            .unwrap_or_default();

        lsp_types::Diagnostic {
            range,
            severity: Some(lsp_types::DiagnosticSeverity::ERROR),
            code: Some(lsp_types::NumberOrString::String(
                self.diagnostic_code().to_string(),
            )),
            code_description: None,
            source: Some("Django Language Server".to_string()),
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

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(transparent)]
pub struct LspRange(pub lsp_types::Range);

impl From<(&Span, &LineIndex)> for LspRange {
    #[inline]
    fn from((s, line_index): (&Span, &LineIndex)) -> Self {
        let start = LspPosition::from((s.start_offset(), line_index)).into();
        let end = LspPosition::from((s.end_offset(), line_index)).into();

        LspRange(lsp_types::Range { start, end })
    }
}

impl From<LspRange> for lsp_types::Range {
    #[inline]
    fn from(value: LspRange) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(transparent)]
pub struct LspPosition(pub lsp_types::Position);

impl From<(Offset, &LineIndex)> for LspPosition {
    #[inline]
    fn from((offset, line_index): (Offset, &LineIndex)) -> Self {
        let (line, character) = line_index.to_line_col(offset).into();
        Self(lsp_types::Position { line, character })
    }
}

impl From<LspPosition> for lsp_types::Position {
    #[inline]
    fn from(value: LspPosition) -> Self {
        value.0
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

//! Diagnostic collection and conversion for IDE features

use djls_templates::ast::Span;

/// Internal diagnostic representation (no LSP types)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdeDiagnostic {
    pub message: String,
    pub severity: DiagnosticSeverity,
    pub span: Span,
    pub code: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

/// Collect all diagnostics for a file from parsing and semantic analysis
pub fn collect_diagnostics(
    db: &dyn djls_workspace::Db,
    file: djls_workspace::SourceFile,
) -> Vec<IdeDiagnostic> {
    let mut diagnostics = vec![];

    // TODO: Collect syntax diagnostics from djls-templates
    // TODO: Collect semantic diagnostics from djls-semantic

    diagnostics
}
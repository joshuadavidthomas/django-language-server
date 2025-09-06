//! Template-specific database trait and queries.
//!
//! This module extends the workspace database trait with template-specific
//! functionality including parsing and diagnostic generation.

use std::sync::Arc;

use djls_workspace::db::SourceFile;
use djls_workspace::Db as WorkspaceDb;
use djls_workspace::FileKind;
use tower_lsp_server::lsp_types;

use crate::ast::LineOffsets;
use crate::ast::Span;
use crate::Ast;
use crate::TemplateError;

/// Template-specific database trait extending the workspace database
#[salsa::db]
pub trait Db: WorkspaceDb {
    // Template-specific methods can be added here if needed
}

/// Container for a parsed Django template AST.
///
/// Stores both the parsed AST and any errors encountered during parsing.
/// This struct is designed to be cached by Salsa and shared across multiple consumers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedTemplate {
    /// The parsed AST from djls-templates
    pub ast: Ast,
    /// Any errors encountered during parsing
    pub errors: Vec<TemplateError>,
}

/// Parse a Django template file into an AST.
///
/// This Salsa tracked function parses template files on-demand and caches the results.
/// The parse is only re-executed when the file's content changes (detected via revision changes).
///
/// Returns `None` for non-template files.
#[salsa::tracked]
pub fn parse_template(db: &dyn Db, file: SourceFile) -> Option<Arc<ParsedTemplate>> {
    // Only parse template files
    if file.kind(db) != FileKind::Template {
        return None;
    }

    let text_arc = djls_workspace::db::source_text(db, file);
    let text = text_arc.as_ref();

    // Call the pure parsing function
    match crate::parse_template(text) {
        Ok((ast, errors)) => Some(Arc::new(ParsedTemplate { ast, errors })),
        Err(err) => {
            // Even on fatal errors, return an empty AST with the error
            Some(Arc::new(ParsedTemplate {
                ast: Ast::default(),
                errors: vec![err],
            }))
        }
    }
}

/// Generate LSP diagnostics for a template file.
///
/// This Salsa tracked function computes diagnostics from template parsing errors
/// and caches the results. Diagnostics are only recomputed when the file changes.
#[salsa::tracked]
pub fn template_diagnostics(db: &dyn Db, file: SourceFile) -> Arc<Vec<lsp_types::Diagnostic>> {
    // Parse the template to get errors
    let Some(parsed) = parse_template(db, file) else {
        return Arc::new(Vec::new());
    };

    if parsed.errors.is_empty() {
        return Arc::new(Vec::new());
    }

    // Convert errors to diagnostics
    let line_offsets = parsed.ast.line_offsets();
    let diagnostics = parsed
        .errors
        .iter()
        .map(|error| template_error_to_diagnostic(error, line_offsets))
        .collect();

    Arc::new(diagnostics)
}

/// Convert a [`TemplateError`] to an LSP [`Diagnostic`].
///
/// Maps template parsing and validation errors to LSP diagnostics with appropriate
/// severity levels, ranges, and metadata.
fn template_error_to_diagnostic(
    error: &TemplateError,
    line_offsets: &LineOffsets,
) -> lsp_types::Diagnostic {
    let severity = severity_from_error(error);
    let range = error
        .span()
        .map(|span| span_to_range(span, line_offsets))
        .unwrap_or_default();

    lsp_types::Diagnostic {
        range,
        severity: Some(severity),
        code: Some(lsp_types::NumberOrString::String(error.code().to_string())),
        code_description: None,
        source: Some("Django Language Server".to_string()),
        message: error.to_string(),
        related_information: None,
        tags: None,
        data: None,
    }
}

/// Map a [`TemplateError`] to appropriate diagnostic severity.
fn severity_from_error(error: &TemplateError) -> lsp_types::DiagnosticSeverity {
    match error {
        TemplateError::Lexer(_) | TemplateError::Parser(_) | TemplateError::Io(_) => {
            lsp_types::DiagnosticSeverity::ERROR
        }
        TemplateError::Validation(_) | TemplateError::Config(_) => {
            lsp_types::DiagnosticSeverity::WARNING
        }
    }
}

/// Convert a template [`Span`] to an LSP [`Range`] using line offsets.
#[allow(clippy::cast_possible_truncation)]
fn span_to_range(span: Span, line_offsets: &LineOffsets) -> lsp_types::Range {
    let start_pos = span.start() as usize;
    let end_pos = (span.start() + span.length()) as usize;

    let (start_line, start_char) = line_offsets.position_to_line_col(start_pos);
    let (end_line, end_char) = line_offsets.position_to_line_col(end_pos);

    // Note: These casts are safe in practice as line numbers and character positions
    // in source files won't exceed u32::MAX (4 billion lines/characters)
    lsp_types::Range {
        start: lsp_types::Position {
            line: (start_line - 1) as u32, // LSP is 0-based, LineOffsets is 1-based
            character: start_char as u32,
        },
        end: lsp_types::Position {
            line: (end_line - 1) as u32, // LSP is 0-based, LineOffsets is 1-based
            character: end_char as u32,
        },
    }
}

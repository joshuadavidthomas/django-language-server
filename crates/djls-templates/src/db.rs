//! Template-specific database trait and queries.
//!
//! This module extends the workspace database trait with template-specific
//! functionality including parsing and diagnostic generation using Salsa accumulators.

use std::sync::Arc;

use djls_workspace::db::SourceFile;
use djls_workspace::Db as WorkspaceDb;
use djls_workspace::FileKind;
use salsa::Accumulator;
use tower_lsp_server::lsp_types;

use crate::ast::AstError;
use crate::ast::LineOffsets;
use crate::ast::Span;
use crate::tagspecs::TagSpecs;
use crate::validation::TagMatcher;
use crate::Ast;
use crate::TemplateError;

/// Diagnostic severity levels
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
}

/// Accumulator for template diagnostics - can be a struct directly!
#[salsa::accumulator]
#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub severity: DiagnosticSeverity,
    pub code: String,
    pub message: String,
    pub span: Option<Span>,
}

/// Template-specific database trait extending the workspace database
#[salsa::db]
pub trait Db: WorkspaceDb {
    /// Get the Django tag specifications for template parsing and validation
    fn tag_specs(&self) -> Arc<TagSpecs>;
}

/// Generate a specific error code for an AstError variant
fn ast_error_to_code(error: &AstError) -> String {
    match error {
        AstError::EmptyAst => "DTL-001".to_string(),
        AstError::InvalidTagStructure { .. } => "DTL-002".to_string(),
        AstError::UnbalancedStructure { .. } => "DTL-003".to_string(),
        AstError::InvalidNode { .. } => "DTL-004".to_string(),
        AstError::UnclosedTag { .. } => "DTL-005".to_string(),
        AstError::OrphanedTag { .. } => "DTL-006".to_string(),
        AstError::UnmatchedBlockName { .. } => "DTL-007".to_string(),
        AstError::MissingRequiredArguments { .. } => "DTL-008".to_string(),
        AstError::TooManyArguments { .. } => "DTL-009".to_string(),
    }
}

/// Generate an error code for parser errors
fn parser_error_to_code() -> String {
    "DTL-100".to_string()
}

/// Generate an error code for lexer errors  
fn lexer_error_to_code() -> String {
    "DTL-200".to_string()
}

/// Parse and validate a Django template file.
///
/// This Salsa tracked function parses template files on-demand and caches the results.
/// During parsing and validation, diagnostics are accumulated using the TemplateDiagnostic
/// accumulator. The function returns the parsed AST (or None for non-template files).
///
/// Diagnostics can be retrieved using:
/// ```ignore
/// let diagnostics = 
///     parse_and_validate_template::accumulated::<Diagnostic>(db, file);
/// ```
#[salsa::tracked]
pub fn parse_and_validate_template(db: &dyn Db, file: SourceFile) -> Option<Arc<Ast>> {
    // Only parse template files
    if file.kind(db) != FileKind::Template {
        return None;
    }

    let text_arc = djls_workspace::db::source_text(db, file);
    let text = text_arc.as_ref();

    // Parse the template
    let (ast, parse_errors) = match crate::parse_template(text) {
        Ok((ast, errors)) => (ast, errors),
        Err(err) => {
            // Fatal parse error - accumulate it and return empty AST
            let code = match &err {
                TemplateError::Lexer(_) => lexer_error_to_code(),
                TemplateError::Parser(_) => parser_error_to_code(),
                _ => "DTL-999".to_string(),
            };
            // Extract just the error message without the prefix
            let message = match &err {
                TemplateError::Lexer(msg) => msg.clone(),
                TemplateError::Parser(msg) => msg.clone(),
                _ => err.to_string(),
            };
            Diagnostic {
                severity: DiagnosticSeverity::Error,
                code,
                message,
                span: err.span(),
            }
            .accumulate(db);
            return Some(Arc::new(Ast::default()));
        }
    };

    // Accumulate parse errors
    for error in parse_errors {
        let code = match &error {
            TemplateError::Lexer(_) => lexer_error_to_code(),
            TemplateError::Parser(_) => parser_error_to_code(),
            _ => "DJ999".to_string(),
        };
        // Extract just the error message without the prefix
        let message = match &error {
            TemplateError::Lexer(msg) => msg.clone(),
            TemplateError::Parser(msg) => msg.clone(),
            _ => error.to_string(),
        };
        Diagnostic {
            severity: DiagnosticSeverity::Error,
            code,
            message,
            span: error.span(),
        }
        .accumulate(db);
    }

    // Perform validation and accumulate errors
    let tag_specs = db.tag_specs();
    let (_, validation_errors) = TagMatcher::match_tags(ast.nodelist(), tag_specs);

    for error in validation_errors {
        Diagnostic {
            severity: DiagnosticSeverity::Error,
            code: ast_error_to_code(&error),
            message: error.to_string(),
            span: error.span(),
        }
        .accumulate(db);
    }

    Some(Arc::new(ast))
}

/// Convert a [`Diagnostic`] to an LSP [`Diagnostic`].
///
/// Maps template diagnostics to LSP diagnostics with appropriate
/// severity levels, ranges, and metadata.
pub fn diagnostic_to_lsp(diag: &Diagnostic, line_offsets: &LineOffsets) -> lsp_types::Diagnostic {
    let range = diag
        .span
        .map(|span| {
            // For validation errors (which are Django tags), adjust the span to include delimiters
            let adjusted_span = if diag.code.starts_with("DTL-") && diag.code != "DTL-100" && diag.code != "DTL-200" {
                // Django tags: add delimiter lengths
                // The stored span only includes content length, so add:
                // - 2 for opening {%
                // - 1 for space after {%
                // - content (already in span.length())
                // - 1 for space before %}
                // - 2 for closing %}
                // Total: 6 extra characters
                let start = span.start();
                let length = span.length() + 6;
                Span::new(start, length)
            } else {
                span
            };
            span_to_range(adjusted_span, line_offsets)
        })
        .unwrap_or_default();

    lsp_types::Diagnostic {
        range,
        severity: Some(match diag.severity {
            DiagnosticSeverity::Error => lsp_types::DiagnosticSeverity::ERROR,
            DiagnosticSeverity::Warning => lsp_types::DiagnosticSeverity::WARNING,
        }),
        code: Some(lsp_types::NumberOrString::String(diag.code.clone())),
        code_description: None,
        source: Some("Django Language Server".to_string()),
        message: diag.message.clone(),
        related_information: None,
        tags: None,
        data: None,
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
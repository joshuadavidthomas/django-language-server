//! Template-specific database trait and queries.
//!
//! This module extends the workspace database trait with template-specific
//! functionality including parsing and diagnostic generation.

use std::sync::Arc;

use djls_workspace::db::SourceFile;
use djls_workspace::Db as WorkspaceDb;
use djls_workspace::FileKind;
use tower_lsp_server::lsp_types;

use crate::ast::AstError;
use crate::ast::LineOffsets;
use crate::ast::Node;
use crate::ast::Span;
use crate::tagspecs::TagSpecs;
use crate::validation::TagInfo;
use crate::validation::TagMatcher;
use crate::validation::TagPairs;
use crate::Ast;
use crate::TemplateError;

/// Template-specific database trait extending the workspace database
#[salsa::db]
pub trait Db: WorkspaceDb {
    /// Get the Django tag specifications for template parsing and validation
    fn tag_specs(&self) -> Arc<TagSpecs>;
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

/// Extract tag structure from AST for validation
#[salsa::tracked]
pub fn template_tag_structure(db: &dyn Db, file: SourceFile) -> Arc<Vec<TagInfo>> {
    let Some(parsed) = parse_template(db, file) else {
        return Arc::new(Vec::new());
    };

    let mut tags = Vec::new();
    for (idx, node) in parsed.ast.nodelist().iter().enumerate() {
        if let Node::Tag { name, bits, span } = node {
            tags.push(TagInfo {
                name: name.clone(),
                bits: bits.clone(),
                span: *span,
                node_index: idx,
            });
        }
    }

    Arc::new(tags)
}

/// Match opening and closing tags using stack algorithm
#[salsa::tracked]
pub fn template_tag_pairs(db: &dyn Db, file: SourceFile) -> Arc<TagPairs> {
    let Some(parsed) = parse_template(db, file) else {
        return Arc::new(TagPairs {
            matched_pairs: Vec::new(),
            unclosed_tags: Vec::new(),
            unexpected_closers: Vec::new(),
            mismatched_pairs: Vec::new(),
            orphaned_intermediates: Vec::new(),
        });
    };

    let tag_specs = db.tag_specs();
    let (pairs, _) = TagMatcher::match_tags(parsed.ast.nodelist(), tag_specs);
    Arc::new(pairs)
}

/// Generate validation errors from tag matching
#[salsa::tracked]
pub fn template_validation_errors(db: &dyn Db, file: SourceFile) -> Arc<Vec<AstError>> {
    let Some(parsed) = parse_template(db, file) else {
        return Arc::new(Vec::new());
    };

    let tag_specs = db.tag_specs();
    let (_, errors) = TagMatcher::match_tags(parsed.ast.nodelist(), tag_specs);
    Arc::new(errors)
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

    let mut all_errors = Vec::new();

    // Add parse errors
    for error in &parsed.errors {
        all_errors.push(error.clone());
    }

    // Add validation errors
    let validation_errors = template_validation_errors(db, file);
    for ast_error in validation_errors.iter() {
        all_errors.push(TemplateError::Validation(ast_error.clone()));
    }

    if all_errors.is_empty() {
        return Arc::new(Vec::new());
    }

    // Convert errors to diagnostics
    let line_offsets = parsed.ast.line_offsets();
    let diagnostics = all_errors
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

    // For validation errors (which are Django tags), adjust the span to include delimiters
    let range = error
        .span()
        .map(|span| {
            let adjusted_span = if matches!(error, TemplateError::Validation(_)) {
                // Django tags: the token start is already at the '{' character
                // We need to add the delimiter lengths and spaces to the span length
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
        TemplateError::Validation(_) => lsp_types::DiagnosticSeverity::ERROR,
        TemplateError::Config(_) => lsp_types::DiagnosticSeverity::WARNING,
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

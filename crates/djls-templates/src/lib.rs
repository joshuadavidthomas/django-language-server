//! Django template parsing, validation, and diagnostics.
//!
//! This crate provides comprehensive support for Django template files including:
//! - Lexical analysis and tokenization
//! - Parsing into an Abstract Syntax Tree (AST)
//! - Validation using configurable tag specifications
//! - LSP diagnostic generation with Salsa integration
//!
//! ## Architecture
//!
//! The system uses a multi-stage pipeline:
//!
//! 1. **Lexing**: Template text is tokenized into Django constructs (tags, variables, text)
//! 2. **Parsing**: Tokens are parsed into a structured AST
//! 3. **Validation**: The AST is validated using the visitor pattern
//! 4. **Diagnostics**: Errors are converted to LSP diagnostics via Salsa accumulators
//!
//! ## Key Components
//!
//! - [`ast`]: AST node definitions and visitor pattern implementation
//! - [`db`]: Salsa database integration for incremental computation
//! - [`validation`]: Validation rules using the visitor pattern
//! - [`tagspecs`]: Django tag specifications for validation
//!
//! ## Adding New Validation Rules
//!
//! 1. Add the error variant to [`TemplateError`]
//! 2. Implement the check in the validation module
//! 3. Add corresponding tests
//!
//! ## Example
//!
//! ```ignore
//! // For LSP integration with Salsa (primary usage):
//! use djls_templates::db::{analyze_template, TemplateDiagnostic};
//!
//! let ast = analyze_template(db, file);
//! let diagnostics = analyze_template::accumulated::<TemplateDiagnostic>(db, file);
//!
//! // For direct parsing (testing/debugging):
//! use djls_templates::{Lexer, Parser};
//!
//! let tokens = Lexer::new(source).tokenize()?;
//! let mut parser = Parser::new(tokens);
//! let (ast, errors) = parser.parse()?;
//! ```

pub mod ast;
pub mod db;
mod error;
mod lexer;
mod parser;
pub mod tagspecs;
mod tokens;
pub mod validation;

use std::sync::Arc;

pub use ast::Ast;
use ast::LineOffsets;
use ast::Span;
pub use db::Db;
pub use db::TemplateDiagnostic;
use djls_workspace::db::SourceFile;
use djls_workspace::FileKind;
pub use error::QuickFix;
pub use error::TemplateError;
pub use lexer::Lexer;
pub use parser::Parser;
pub use parser::ParserError;
use salsa::Accumulator;
use validation::validate_template;

/// Helper function to convert errors to LSP diagnostics and accumulate
fn accumulate_error(db: &dyn Db, error: &TemplateError, line_offsets: &LineOffsets) {
    let code = error.diagnostic_code();
    let range = error
        .span()
        .map(|span| {
            // For validation errors (which are Django tags), adjust the span to include delimiters
            let adjusted_span =
                if code.starts_with("DTL-") && code != "DTL-100" && code != "DTL-200" {
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
            adjusted_span.to_lsp_range(line_offsets)
        })
        .unwrap_or_default();

    let diagnostic = tower_lsp_server::lsp_types::Diagnostic {
        range,
        severity: Some(tower_lsp_server::lsp_types::DiagnosticSeverity::ERROR),
        code: Some(tower_lsp_server::lsp_types::NumberOrString::String(
            code.to_string(),
        )),
        code_description: None,
        source: Some("Django Language Server".to_string()),
        message: match error {
            TemplateError::Lexer(msg) | TemplateError::Parser(msg) => msg.clone(),
            _ => error.to_string(),
        },
        related_information: None,
        tags: None,
        data: None,
    };

    TemplateDiagnostic(diagnostic).accumulate(db);
}

/// Analyze a Django template file - parse, validate, and accumulate diagnostics.
///
/// This is the PRIMARY function for template processing. It's a Salsa tracked function
/// that parses template files on-demand and caches the results. During parsing and
/// validation, diagnostics are accumulated using the TemplateDiagnostic accumulator.
///
/// The function returns the parsed AST (or None for non-template files).
///
/// Diagnostics can be retrieved using:
/// ```ignore
/// let diagnostics =
///     analyze_template::accumulated::<TemplateDiagnostic>(db, file);
/// ```
#[salsa::tracked]
pub fn analyze_template(db: &dyn Db, file: SourceFile) -> Option<Arc<Ast>> {
    // Only process template files
    if file.kind(db) != FileKind::Template {
        return None;
    }

    let text_arc = djls_workspace::db::source_text(db, file);
    let text = text_arc.as_ref();

    // Tokenize the template
    let tokens = match Lexer::new(text).tokenize() {
        Ok(tokens) => tokens,
        Err(err) => {
            // Fatal lexer error - accumulate it and return empty AST
            let ast = Ast::default();
            let error = TemplateError::Lexer(err.to_string());
            accumulate_error(db, &error, ast.line_offsets());
            return Some(Arc::new(ast));
        }
    };

    // Parse the tokens into AST
    let mut parser = Parser::new(tokens);
    let (ast, parser_errors) = match parser.parse() {
        Ok((ast, errors)) => {
            let template_errors: Vec<TemplateError> = errors
                .into_iter()
                .map(|e| TemplateError::Parser(e.to_string()))
                .collect();
            (ast, template_errors)
        }
        Err(err) => {
            // Fatal parse error - accumulate it and return empty AST
            let ast = Ast::default();
            let error = TemplateError::Parser(err.to_string());
            accumulate_error(db, &error, ast.line_offsets());
            return Some(Arc::new(ast));
        }
    };

    // Accumulate parse errors
    for error in &parser_errors {
        accumulate_error(db, error, ast.line_offsets());
    }

    // Perform validation and accumulate errors
    let tag_specs = db.tag_specs();
    let (_, validation_errors) = validate_template(ast.nodelist(), tag_specs);

    for error in validation_errors {
        // Convert validation error to TemplateError for consistency
        let template_error = TemplateError::Validation(error);
        accumulate_error(db, &template_error, ast.line_offsets());
    }

    Some(Arc::new(ast))
}

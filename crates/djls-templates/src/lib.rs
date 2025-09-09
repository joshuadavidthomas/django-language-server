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
pub mod templatetags;
mod tokens;
pub mod validation;

pub use ast::Ast;
use ast::LineOffsets;
pub use db::Db;
pub use db::TemplateDiagnostic;
use djls_workspace::db::SourceFile;
use djls_workspace::FileKind;
pub use error::TemplateError;
pub use lexer::Lexer;
pub use parser::Parser;
pub use parser::ParserError;
use salsa::Accumulator;
use tokens::TokenStream;
use validation::TagValidator;

/// Lex a template file into tokens.
///
/// This is the first phase of template processing. It tokenizes the source text
/// into Django-specific tokens (tags, variables, text, etc.).
#[salsa::tracked]
fn lex_template(db: &dyn Db, file: SourceFile) -> TokenStream<'_> {
    if file.kind(db) != FileKind::Template {
        return TokenStream::new(db, vec![]);
    }

    let text_arc = djls_workspace::db::source_text(db, file);
    let text = text_arc.as_ref();

    match Lexer::new(text).tokenize() {
        Ok(tokens) => TokenStream::new(db, tokens),
        Err(err) => {
            // Create error diagnostic
            let error = TemplateError::Lexer(err.to_string());
            let empty_offsets = LineOffsets::default();
            accumulate_error(db, &error, &empty_offsets);

            // Return empty token stream
            TokenStream::new(db, vec![])
        }
    }
}

/// Parse tokens into an AST.
///
/// This is the second phase of template processing. It takes the token stream
/// from lexing and builds an Abstract Syntax Tree.
#[salsa::tracked]
fn parse_template(db: &dyn Db, file: SourceFile) -> Ast<'_> {
    let token_stream = lex_template(db, file);

    // Check if lexing produced no tokens (likely due to an error)
    if token_stream.stream(db).is_empty() {
        // Return empty AST for error recovery
        let empty_nodelist = Vec::new();
        let empty_offsets = LineOffsets::default();
        return Ast::new(db, empty_nodelist, empty_offsets);
    }

    // Parser needs the TokenStream<'db>
    match Parser::new(db, token_stream).parse() {
        Ok((ast, errors)) => {
            // Accumulate parser errors
            for error in errors {
                let template_error = TemplateError::Parser(error.to_string());
                accumulate_error(db, &template_error, ast.line_offsets(db));
            }
            ast
        }
        Err(err) => {
            // Critical parser error
            let template_error = TemplateError::Parser(err.to_string());
            let empty_offsets = LineOffsets::default();
            accumulate_error(db, &template_error, &empty_offsets);

            // Return empty AST
            let empty_nodelist = Vec::new();
            let empty_offsets = LineOffsets::default();
            Ast::new(db, empty_nodelist, empty_offsets)
        }
    }
}

/// Validate the AST.
///
/// This is the third phase of template processing. It validates the AST
/// according to Django tag specifications and accumulates any validation errors.
#[salsa::tracked]
fn validate_template(db: &dyn Db, file: SourceFile) {
    let ast = parse_template(db, file);

    // Skip validation if AST is empty (likely due to parse errors)
    if ast.nodelist(db).is_empty() && lex_template(db, file).stream(db).is_empty() {
        return;
    }

    let validation_errors = TagValidator::new(db, ast).validate();

    for error in validation_errors {
        // Convert validation error to TemplateError for consistency
        let template_error = TemplateError::Validation(error);
        accumulate_error(db, &template_error, ast.line_offsets(db));
    }
}

/// Helper function to convert errors to LSP diagnostics and accumulate
fn accumulate_error(db: &dyn Db, error: &TemplateError, line_offsets: &LineOffsets) {
    let code = error.diagnostic_code();
    let range = error
        .span()
        .map(|(start, length)| {
            let span = crate::ast::Span::new(start, length);
            span.to_lsp_range(line_offsets)
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
/// that orchestrates the three phases of template processing:
/// 1. Lexing (tokenization)
/// 2. Parsing (AST construction)
/// 3. Validation (semantic checks)
///
/// Each phase is independently cached by Salsa, allowing for fine-grained
/// incremental computation.
///
/// The function returns the parsed AST (or None for non-template files).
///
/// Diagnostics can be retrieved using:
/// ```ignore
/// let diagnostics =
///     analyze_template::accumulated::<TemplateDiagnostic>(db, file);
/// ```
#[salsa::tracked]
pub fn analyze_template(db: &dyn Db, file: SourceFile) -> Option<Ast<'_>> {
    if file.kind(db) != FileKind::Template {
        return None;
    }
    validate_template(db, file);
    Some(parse_template(db, file))
}

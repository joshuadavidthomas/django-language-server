//! Django template parsing, validation, and diagnostics.
//!
//! This crate provides comprehensive support for Django template files including:
//! - Lexical analysis and tokenization
//! - Parsing into a flat node list
//! - Validation using configurable tag specifications
//! - LSP diagnostic generation with Salsa integration
//!
//! ## Architecture
//!
//! The system uses a multi-stage pipeline:
//!
//! 1. **Lexing**: Template text is tokenized into Django constructs (tags, variables, text)
//! 2. **Parsing**: Tokens are parsed into a flat node list
//! 3. **Validation**: The node list is validated using the visitor pattern
//! 4. **Diagnostics**: Errors are converted to LSP diagnostics via Salsa accumulators
//!
//! ## Key Components
//!
//! - [`nodelist`]: Node list definitions and structure
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
//! use djls_templates::{parse_template, TemplateDiagnostic};
//!
//! let nodelist = parse_template(db, file);
//! let diagnostics = parse_template::accumulated::<TemplateDiagnostic>(db, file);
//!
//! // For direct parsing (testing/debugging):
//! use djls_templates::{Lexer, Parser};
//!
//! let tokens = Lexer::new(source).tokenize()?;
//! let mut parser = Parser::new(tokens);
//! let (nodelist, errors) = parser.parse()?;
//! ```

pub mod db;
mod error;
mod lexer;
pub mod nodelist;
mod parser;
mod tokens;

pub use db::Db;
pub use db::TemplateDiagnostic;
use djls_workspace::db::SourceFile;
use djls_workspace::FileKind;
pub use error::TemplateError;
pub use lexer::Lexer;
use nodelist::LineOffsets;
pub use nodelist::NodeList;
pub use parser::Parser;
pub use parser::ParserError;
use salsa::Accumulator;
use tokens::TokenStream;

/// Lex a template file into tokens.
#[salsa::tracked]
fn lex_template(db: &dyn Db, file: SourceFile) -> TokenStream<'_> {
    if file.kind(db) != FileKind::Template {
        return TokenStream::new(db, vec![], LineOffsets::default());
    }

    let text_arc = djls_workspace::db::source_text(db, file);
    let text = text_arc.as_ref();

    let (tokens, line_offsets) = Lexer::new(db, text).tokenize();
    TokenStream::new(db, tokens, line_offsets)
}

/// Parse a Django template file and accumulate diagnostics.
///
/// Diagnostics can be retrieved using:
/// ```ignore
/// let diagnostics =
///     parse_template::accumulated::<TemplateDiagnostic>(db, file);
/// ```
#[salsa::tracked]
pub fn parse_template(db: &dyn Db, file: SourceFile) -> Option<NodeList<'_>> {
    if file.kind(db) != FileKind::Template {
        return None;
    }

    let token_stream = lex_template(db, file);

    if token_stream.stream(db).is_empty() {
        let empty_nodelist = Vec::new();
        let empty_offsets = LineOffsets::default();
        return Some(NodeList::new(db, empty_nodelist, empty_offsets));
    }

    let nodelist = match Parser::new(db, token_stream).parse() {
        Ok((nodelist, errors)) => {
            for error in errors {
                let template_error = TemplateError::Parser(error.to_string());
                accumulate_error(db, &template_error, nodelist.line_offsets(db));
            }
            nodelist
        }
        Err(err) => {
            let template_error = TemplateError::Parser(err.to_string());
            let empty_offsets = LineOffsets::default();
            accumulate_error(db, &template_error, &empty_offsets);

            let empty_nodelist = Vec::new();
            let empty_offsets = LineOffsets::default();
            NodeList::new(db, empty_nodelist, empty_offsets)
        }
    };

    Some(nodelist)
}

fn accumulate_error(db: &dyn Db, error: &TemplateError, line_offsets: &LineOffsets) {
    let code = error.diagnostic_code();
    let range = error
        .span()
        .map(|(start, length)| {
            let span = crate::nodelist::Span::new(start, length);
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
            TemplateError::Parser(msg) => msg.clone(),
            _ => error.to_string(),
        },
        related_information: None,
        tags: None,
        data: None,
    };

    TemplateDiagnostic(diagnostic).accumulate(db);
}

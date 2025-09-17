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
//! use djls_templates::{parse_template, TemplateErrorAccumulator};
//!
//! let nodelist = parse_template(db, file);
//! let errors = parse_template::accumulated::<TemplateErrorAccumulator>(db, file);
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
pub use db::TemplateErrorAccumulator;
use djls_source::File;
use djls_source::FileKind;
pub use error::TemplateError;
pub use lexer::Lexer;
pub use nodelist::ErrorNode;
use nodelist::Node;
pub use nodelist::NodeList;
pub use parser::ParseError;
pub use parser::Parser;
use salsa::Accumulator;
use tokens::TokenStream;

/// Lex a template file into tokens.
#[salsa::tracked]
fn lex_template(db: &dyn Db, file: File) -> TokenStream<'_> {
    let source = file.source(db);
    if *source.kind() != FileKind::Template {
        return TokenStream::new(db, vec![]);
    }
    let text = source.as_ref();
    let tokens = Lexer::new(db, text).tokenize();
    TokenStream::new(db, tokens)
}

/// Parse a Django template file and accumulate diagnostics.
///
/// Diagnostics can be retrieved using:
/// ```ignore
/// let diagnostics =
///     parse_template::accumulated::<TemplateDiagnostic>(db, file);
/// ```
#[salsa::tracked]
pub fn parse_template(db: &dyn Db, file: File) -> Option<NodeList<'_>> {
    let source = file.source(db);
    if *source.kind() != FileKind::Template {
        return None;
    }

    let token_stream = lex_template(db, file);

    if token_stream.stream(db).is_empty() {
        let empty_nodelist = Vec::new();
        return Some(NodeList::new(db, empty_nodelist));
    }

    let nodelist = match Parser::new(db, token_stream).parse() {
        Ok(nodelist) => nodelist,
        Err(err) => {
            // Fatal error - accumulate but still return an error node so spans remain intact
            let template_error = TemplateError::Parser(err.to_string());
            TemplateErrorAccumulator(template_error).accumulate(db);

            let text = source.as_ref();
            let span = djls_source::Span::from_bounds(0, text.len());
            let error_node = Node::Error {
                node: ErrorNode {
                    span,
                    full_span: span,
                    error: err,
                },
            };

            NodeList::new(db, vec![error_node])
        }
    };

    Some(nodelist)
}

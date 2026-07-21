//! Django template syntax parsing.
//!
//! This crate provides Django template lexing and parsing:
//! - Lexical analysis and tokenization
//! - Parsing into a flat node list
//! - Error recovery for incomplete editor input
//! - Parse diagnostics through a Salsa accumulator
//!
//! ## Architecture
//!
//! The system uses a multi-stage pipeline:
//!
//! 1. **Lexing**: Template text is tokenized into Django constructs (tags, variables, text)
//! 2. **Parsing**: Tokens are parsed into a flat node list
//! 3. **Diagnostics**: Parse errors are emitted through a Salsa accumulator
//!
//! ## Key Components
//!
//! - [`NodeList`]: Parsed template nodes
//! - [`Node`]: Individual parsed template node
//! - [`TemplateErrorAccumulator`]: Salsa accumulator for parse errors
//!
//! ## Example
//!
//! ```ignore
//! // For LSP integration with Salsa (primary usage):
//! use djls_templates::{parse_template, TemplateErrorAccumulator};
//!
//! let nodelist = parse_template(db, file).expect("template should be readable");
//! let errors = parse_template::accumulated::<TemplateErrorAccumulator>(db, file);
//!
//! // For direct parsing (testing/debugging):
//! use djls_templates::parse_template_impl;
//!
//! let (nodelist, errors) = parse_template_impl(source);
//! ```

mod bits;
mod db;
mod error;
mod filters;
mod lexer;
mod nodelist;
mod parser;
mod quotes;
mod tokens;
mod visitor;

pub use bits::FilterArgument;
pub use bits::TagBit;
pub use db::TemplateErrorAccumulator;
use djls_source::Db;
use djls_source::File;
use djls_source::FileKind;
use djls_source::FileReadError;
pub use error::TemplateError;
pub use filters::Filter;
pub use nodelist::Node;
pub use nodelist::NodeList;
pub use parser::ParseError;
pub use quotes::TemplateString;
use salsa::Accumulator;
pub use tokens::TagDelimiter;
pub use tokens::Token;
pub use visitor::Visitor;

#[derive(Clone, PartialEq, Eq, salsa::SalsaValue)]
pub enum TemplateParseResult<'db> {
    Parsed(NodeList<'db>),
    NotTemplate,
    Unreadable(FileReadError),
}

impl<'db> TemplateParseResult<'db> {
    /// Return the parsed tree or panic with the supplied fixture context.
    ///
    /// This is intended for callers that have already established that the
    /// file is a readable template, such as tests and benchmarks.
    ///
    /// # Panics
    ///
    /// Panics when the file is not a template or could not be read.
    #[track_caller]
    #[must_use]
    pub fn expect(self, message: &str) -> NodeList<'db> {
        match self {
            Self::Parsed(nodelist) => nodelist,
            Self::NotTemplate => panic!("{message}: file is not a template"),
            Self::Unreadable(error) => panic!("{message}: {error}"),
        }
    }
}

/// Lex a Django template file.
#[salsa::tracked(returns(ref))]
pub fn lex_template(db: &dyn Db, file: File) -> Result<Vec<Token>, FileReadError> {
    let source = file.try_source(db)?;
    if *source.kind() != FileKind::Template {
        return Ok(Vec::new());
    }

    Ok(lex_template_impl(source.as_ref()))
}

/// Lex a template using the pure lexer.
#[must_use]
pub fn lex_template_impl(source: &str) -> Vec<Token> {
    let mut lexer = lexer::Lexer::new(source);
    lexer.tokenize()
}

/// Parse a Django template file and accumulate diagnostics.
///
/// Diagnostics can be retrieved using:
/// ```ignore
/// let diagnostics =
///     parse_template::accumulated::<TemplateDiagnostic>(db, file);
/// ```
#[salsa::tracked(returns(clone))]
pub fn parse_template(db: &dyn Db, file: File) -> TemplateParseResult<'_> {
    let source = match file.try_source(db) {
        Ok(source) => source,
        Err(error) => return TemplateParseResult::Unreadable(error),
    };
    if *source.kind() != FileKind::Template {
        return TemplateParseResult::NotTemplate;
    }

    let (nodes, errors) = parse_template_impl(source.as_ref());

    // Accumulate any errors via Salsa
    for error in errors {
        TemplateErrorAccumulator(error.into()).accumulate(db);
    }

    // Always return a NodeList (may contain Error nodes if there were parse errors)
    TemplateParseResult::Parsed(NodeList::new(db, nodes))
}

/// Parse a template using the pure parser (no database needed)
/// Returns a tuple of (nodes, errors) where nodes include Error nodes for parse errors
#[must_use]
pub fn parse_template_impl(source: &str) -> (Vec<Node>, Vec<ParseError>) {
    let tokens = lex_template_impl(source);
    let mut parser = parser::Parser::new(tokens);
    parser.parse()
}

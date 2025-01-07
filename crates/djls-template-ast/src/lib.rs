mod ast;
mod lexer;
mod parser;
mod tagspecs;
mod tokens;

pub use ast::Ast;
pub use parser::{Parser, ParserError};
use lexer::Lexer;
use parser::Parser;
use tagspecs::TagSpecs;

/// Parses a Django template and returns the AST and any parsing errors.
///
/// - `source`: The template source code as a `&str`.
/// - `tag_specs`: Optional `TagSpecs` to use for parsing (e.g., custom tags).
///
/// Returns a `Result` containing a tuple of `(Ast, Vec<ParserError>)` on success,
/// or a `ParserError` on failure.
pub fn parse_template(
    source: &str,
    tag_specs: Option<&TagSpecs>,
) -> Result<(Ast, Vec<ParserError>), ParserError> {
    // Tokenize the source using the Lexer
    let tokens = Lexer::new(source).tokenize()?;

    // Use provided TagSpecs or load builtin ones
    let tag_specs = match tag_specs {
        Some(specs) => specs.clone(),
        None => TagSpecs::load_builtin_specs()?,
    };

    // Parse the tokens into an AST using the Parser
    let mut parser = Parser::new(tokens, tag_specs);
    parser.parse()
}

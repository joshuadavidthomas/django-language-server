mod ast;
mod lexer;
mod parser;
mod tagspecs;
mod tokens;

pub use ast::Ast;
use lexer::Lexer;
pub use parser::{Parser, ParserError};
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
    let tokens = Lexer::new(source).tokenize()?;

    let tag_specs = match tag_specs {
        Some(specs) => specs.clone(),
        None => TagSpecs::load_builtin_specs()?,
    };

    let mut parser = Parser::new(tokens, tag_specs);
    parser.parse()
}

mod ast;
mod error;
mod lexer;
mod parser;
mod tagspecs;
mod tokens;

use ast::NodeList;
pub use error::{to_lsp_diagnostic, QuickFix, TemplateError};

use lexer::Lexer;
pub use parser::{Parser, ParserError};

/// Parses a Django template and returns the AST and any parsing errors.
///
/// - `source`: The template source code as a `&str`.
/// - `tag_specs`: Optional `TagSpecs` to use for parsing (e.g., custom tags).
///
/// Returns a `Result` containing a tuple of `(Ast, Vec<ParserError>)` on success,
/// or a `ParserError` on failure.
pub fn parse_template(source: &str) -> Result<(NodeList, Vec<TemplateError>), TemplateError> {
    let tokens = Lexer::new(source)
        .tokenize()
        .map_err(|e| TemplateError::Lexer(e.to_string()))?;

    // let tag_specs = match tag_specs {
    //     Some(specs) => specs.clone(),
    //     None => TagSpecs::load_builtin_specs()
    //         .map_err(|e| TemplateError::Config(format!("Failed to load builtin specs: {}", e)))?,
    // };

    let mut parser = Parser::new(tokens);
    let (nodelist, parser_errors) = parser
        .parse()
        .map_err(|e| TemplateError::Parser(e.to_string()))?;

    // Convert parser errors to TemplateError
    let all_errors = parser_errors
        .into_iter()
        .map(|e| TemplateError::Parser(e.to_string()))
        .collect::<Vec<_>>();

    Ok((nodelist, all_errors))
}

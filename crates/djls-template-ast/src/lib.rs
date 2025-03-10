mod ast;
mod error;
mod lexer;
mod parser;
mod tagspecs;
mod tokens;
mod validator;

pub use error::{to_lsp_diagnostic, QuickFix, TemplateError};

pub use ast::Ast;
use lexer::Lexer;
pub use parser::{Parser, ParserError};
use tagspecs::TagSpecs;
use validator::Validator;

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
) -> Result<(Ast, Vec<TemplateError>), TemplateError> {
    let tokens = Lexer::new(source)
        .tokenize()
        .map_err(|e| TemplateError::Lexer(e.to_string()))?;

    let tag_specs = match tag_specs {
        Some(specs) => specs.clone(),
        None => TagSpecs::load_builtin_specs()
            .map_err(|e| TemplateError::Config(format!("Failed to load builtin specs: {}", e)))?,
    };

    let mut parser = Parser::new(tokens, tag_specs.clone());
    let (ast, parser_errors) = parser
        .parse()
        .map_err(|e| TemplateError::Parser(e.to_string()))?;

    // Convert parser errors to TemplateError
    let mut all_errors = parser_errors
        .into_iter()
        .map(|e| TemplateError::Parser(e.to_string()))
        .collect::<Vec<_>>();

    // Run validation
    let mut validator = Validator::new(&ast, &tag_specs);
    let validation_errors = validator.validate();
    all_errors.extend(validation_errors.into_iter().map(TemplateError::Validation));

    Ok((ast, all_errors))
}

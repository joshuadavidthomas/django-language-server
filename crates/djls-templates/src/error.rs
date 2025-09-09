use serde::Serialize;
use thiserror::Error;

use crate::ast::AstError;
use crate::lexer::LexerError;
use crate::parser::ParserError;

#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize)]
pub enum TemplateError {
    #[error("{0}")]
    Lexer(String),

    #[error("{0}")]
    Parser(String),

    #[error("{0}")]
    Validation(#[from] AstError),

    #[error("IO error: {0}")]
    Io(String),

    #[error("Configuration error: {0}")]
    Config(String),
}

impl From<LexerError> for TemplateError {
    fn from(err: LexerError) -> Self {
        Self::Lexer(err.to_string())
    }
}

impl From<ParserError> for TemplateError {
    fn from(err: ParserError) -> Self {
        Self::Parser(err.to_string())
    }
}

impl From<std::io::Error> for TemplateError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

impl TemplateError {
    #[must_use]
    pub fn span(&self) -> Option<(u32, u32)> {
        match self {
            TemplateError::Validation(ast_error) => ast_error.span(),
            _ => None,
        }
    }

    #[must_use]
    pub fn diagnostic_code(&self) -> &'static str {
        match self {
            TemplateError::Lexer(_) => "T200",
            TemplateError::Parser(_) => "T100",
            TemplateError::Validation(ast_error) => ast_error.diagnostic_code(),
            TemplateError::Io(_) => "T900",
            TemplateError::Config(_) => "T901",
        }
    }
}

use serde::Serialize;
use thiserror::Error;

use crate::ast::AstError;
use crate::ast::Span;
use crate::lexer::LexerError;
use crate::parser::ParserError;

#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize)]
pub enum TemplateError {
    #[error("Lexer error: {0}")]
    Lexer(String),

    #[error("Parser error: {0}")]
    Parser(String),

    #[error("Validation error: {0}")]
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
    pub fn span(&self) -> Option<Span> {
        match self {
            TemplateError::Validation(ast_error) => match ast_error {
                AstError::InvalidTagStructure { span, .. }
                | AstError::InvalidNode { span, .. }
                | AstError::UnclosedTag { span, .. }
                | AstError::OrphanedTag { span, .. }
                | AstError::UnmatchedBlockName { span, .. }
                | AstError::MissingRequiredArguments { span, .. }
                | AstError::TooManyArguments { span, .. } => Some(*span),
                AstError::UnbalancedStructure { opening_span, .. } => Some(*opening_span),
                AstError::EmptyAst => None,
            },
            _ => None,
        }
    }

    /// Get a diagnostic code string for this error type
    #[must_use]
    pub fn diagnostic_code(&self) -> &'static str {
        match self {
            TemplateError::Lexer(_) => "DTL-200",
            TemplateError::Parser(_) => "DTL-100",
            TemplateError::Validation(ast_error) => ast_error.diagnostic_code(),
            TemplateError::Io(_) => "DTL-900",
            TemplateError::Config(_) => "DTL-901",
        }
    }

    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            TemplateError::Lexer(_) => "LEX",
            TemplateError::Parser(_) => "PAR",
            TemplateError::Validation(_) => "VAL",
            TemplateError::Io(_) => "IO",
            TemplateError::Config(_) => "CFG",
        }
    }
}

pub struct QuickFix {
    pub title: String,
    pub edit: String,
}

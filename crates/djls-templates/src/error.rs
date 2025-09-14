use serde::Serialize;
use thiserror::Error;

use crate::nodelist::NodeListError;
use crate::parser::ParserError;

#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize)]
pub enum TemplateError {
    #[error("{0}")]
    Parser(String),

    #[error("{0}")]
    Validation(#[from] NodeListError),

    #[error("IO error: {0}")]
    Io(String),

    #[error("Configuration error: {0}")]
    Config(String),
}

impl From<std::io::Error> for TemplateError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

/// Internal diagnostic representation (no LSP types)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxDiagnostic {
    pub message: String,
    pub span: Span,
    pub code: &'static str,
}

impl TemplateError {
    #[must_use]
    pub fn span(&self) -> Option<(u32, u32)> {
        match self {
            TemplateError::Validation(nodelist_error) => nodelist_error.span(),
            _ => None,
        }
    }

    #[must_use]
    pub fn diagnostic_code(&self) -> &'static str {
        match self {
            TemplateError::Parser(_) => "T100",
            TemplateError::Validation(nodelist_error) => nodelist_error.diagnostic_code(),
            TemplateError::Io(_) => "T900",
            TemplateError::Config(_) => "T901",
        }
    }
}

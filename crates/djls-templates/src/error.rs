use serde::Serialize;
use thiserror::Error;

use crate::parser::ParseError;

#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize)]
pub enum TemplateError {
    #[error("{0}")]
    Parser(String),

    #[error("IO error: {0}")]
    Io(String),

    #[error("Configuration error: {0}")]
    Config(String),
}

impl TemplateError {
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Parser(_) => "T100",
            Self::Io(_) => "T900",
            Self::Config(_) => "T901",
        }
    }

    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Parser(_) => "parser-error",
            Self::Io(_) => "io-error",
            Self::Config(_) => "config-error",
        }
    }
}

impl From<ParseError> for TemplateError {
    fn from(err: ParseError) -> Self {
        Self::Parser(err.to_string())
    }
}

impl From<std::io::Error> for TemplateError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

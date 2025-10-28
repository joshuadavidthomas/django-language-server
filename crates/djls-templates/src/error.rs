use serde::Serialize;
use thiserror::Error;

use crate::parser::ParseError;

#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize)]
pub enum TemplateError {
    /// Parser Error
    ///
    /// This error occurs when the template parser encounters invalid Django template syntax.
    /// Common causes include:
    /// - Malformed template tags (e.g., missing closing braces)
    /// - Invalid variable syntax
    /// - Unclosed strings or comments
    /// - Unexpected characters in template expressions
    ///
    /// # Examples
    ///
    /// ```django
    /// {# This will cause a parser error
    /// {{ variable | filter: invalid }}
    /// ```
    #[diagnostic(code = "T100", category = "template")]
    #[error("{0}")]
    Parser(String),

    /// IO Error
    ///
    /// This error occurs when the language server encounters file system or I/O issues
    /// while processing templates. Common causes include:
    /// - Permission issues reading template files
    /// - Missing or deleted template files
    /// - File system access errors
    /// - Network issues if templates are on remote storage
    #[diagnostic(code = "T900", category = "template")]
    #[error("IO error: {0}")]
    Io(String),

    /// Configuration Error
    ///
    /// This error indicates a problem with the template configuration. Common causes include:
    /// - Invalid settings in Django template configuration
    /// - Misconfigured template loaders
    /// - Issues with template directories
    /// - Invalid template engine settings
    #[diagnostic(code = "T901", category = "template")]
    #[error("Configuration error: {0}")]
    Config(String),
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

use serde::Serialize;
use thiserror::Error;

use crate::parser::ParseError;

#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize)]
pub enum TemplateError {
    #[error("{0}")]
    Parser(ParseError),

    #[error("IO error: {0}")]
    Io(String),

    #[error("Configuration error: {0}")]
    Config(String),
}

impl TemplateError {
    #[must_use]
    pub fn diagnostic_code(&self) -> &'static str {
        match self {
            TemplateError::Parser(_) => "T100",
            TemplateError::Io(_) => "T900",
            TemplateError::Config(_) => "T901",
        }
    }

    #[must_use]
    pub fn primary_span(&self) -> Option<(u32, u32)> {
        match self {
            TemplateError::Parser(error) => parse_error_span(error),
            TemplateError::Io(_) | TemplateError::Config(_) => None,
        }
    }
}

fn parse_error_span(error: &ParseError) -> Option<(u32, u32)> {
    let (position, length) = match error {
        ParseError::UnexpectedToken {
            found, position, ..
        } => (*position, found.len().max(1)),
        ParseError::MissingCondition { tag, position } => (*position, tag.len().max(1)),
        ParseError::MissingIterator { position } => (*position, 1),
        ParseError::MalformedVariable { position, content } => (*position, content.len().max(1)),
        ParseError::InvalidFilterSyntax { position, reason } => (*position, reason.len().max(1)),
        ParseError::UnclosedTag { opener, .. } => (*opener, 1),
        ParseError::InvalidSyntax { position, .. } | ParseError::EmptyTag { position } => {
            (*position, 1)
        }
        ParseError::MalformedConstruct {
            position, opener, ..
        } => (*position, opener.len().max(1)),
        ParseError::StreamError { .. } => {
            return None;
        }
    };

    Some((position.try_into().ok()?, length.try_into().ok()?))
}

impl From<ParseError> for TemplateError {
    fn from(err: ParseError) -> Self {
        Self::Parser(err)
    }
}

impl From<std::io::Error> for TemplateError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_errors_keep_legacy_diagnostic_code() {
        let error = ParseError::MalformedConstruct {
            position: 15,
            opener: "{{".to_string(),
            closer: "}}".to_string(),
            content: "value".to_string(),
        };

        assert_eq!(TemplateError::from(error).diagnostic_code(), "T100");
    }

    #[test]
    fn parser_errors_keep_primary_spans() {
        assert_eq!(
            TemplateError::from(ParseError::MalformedConstruct {
                position: 15,
                opener: "{{".to_string(),
                closer: "}}".to_string(),
                content: "value".to_string(),
            })
            .primary_span(),
            Some((15, 2))
        );
        assert_eq!(
            TemplateError::from(ParseError::UnclosedTag {
                opener: 12,
                expected_closer: "endif".to_string(),
            })
            .primary_span(),
            Some((12, 1))
        );
        assert_eq!(
            TemplateError::from(ParseError::EmptyTag { position: 4 }).primary_span(),
            Some((4, 1))
        );
    }

    #[test]
    fn malformed_construct_span_survives_parse_pipeline() {
        let source = "Hello {{ value";
        let (_, errors) = crate::parse_template_impl(source);

        assert_eq!(errors.len(), 1);
        let error = TemplateError::from(errors.into_iter().next().unwrap());

        assert_eq!(error.diagnostic_code(), "T100");
        assert_eq!(error.primary_span(), Some((6, 2)));
    }
}

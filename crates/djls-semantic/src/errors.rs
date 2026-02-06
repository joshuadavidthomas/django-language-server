use djls_source::Span;
use serde::Serialize;
use thiserror::Error;

#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize)]
pub enum ValidationError {
    #[error("Unclosed tag: {tag}")]
    UnclosedTag { tag: String, span: Span },

    #[error("Orphaned tag '{tag}' - {context}")]
    OrphanedTag {
        tag: String,
        context: String,
        span: Span,
    },

    #[error("Unbalanced structure: '{opening_tag}' missing closing '{expected_closing}'")]
    UnbalancedStructure {
        opening_tag: String,
        expected_closing: String,
        opening_span: Span,
        closing_span: Option<Span>,
    },

    #[error("endblock '{name}' does not match any open block")]
    UnmatchedBlockName { name: String, span: Span },

    #[error("Unknown tag '{tag}'")]
    UnknownTag { tag: String, span: Span },

    #[error("Tag '{tag}' requires {{% load {library} %}}")]
    UnloadedTag {
        tag: String,
        library: String,
        span: Span,
    },

    #[error("Tag '{tag}' is defined in multiple libraries: {libraries:?}")]
    AmbiguousUnloadedTag {
        tag: String,
        libraries: Vec<String>,
        span: Span,
    },

    #[error("Unknown filter '{filter}'")]
    UnknownFilter { filter: String, span: Span },

    #[error("Filter '{filter}' requires {{% load {library} %}}")]
    UnloadedFilter {
        filter: String,
        library: String,
        span: Span,
    },

    #[error("Filter '{filter}' is defined in multiple libraries: {libraries:?}")]
    AmbiguousUnloadedFilter {
        filter: String,
        libraries: Vec<String>,
        span: Span,
    },

    #[error("{message}")]
    ExpressionSyntaxError {
        tag: String,
        message: String,
        span: Span,
    },

    #[error("Filter '{filter}' requires an argument")]
    FilterMissingArgument { filter: String, span: Span },

    #[error("Filter '{filter}' does not accept an argument")]
    FilterUnexpectedArgument { filter: String, span: Span },

    #[error("{message}")]
    ExtractedRuleViolation {
        tag: String,
        message: String,
        span: Span,
    },
}

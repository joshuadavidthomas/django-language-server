// Import Span from templates - we'll need to re-export it or create our own
use djls_templates::Span;
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

    #[error("Tag '{tag}' requires at least {min} argument{}", if *.min == 1 { "" } else { "s" })]
    MissingRequiredArguments { tag: String, min: usize, span: Span },

    #[error("Tag '{tag}' accepts at most {max} argument{}", if *.max == 1 { "" } else { "s" })]
    TooManyArguments { tag: String, max: usize, span: Span },
}

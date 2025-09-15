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

impl ValidationError {
    /// Get the span start and length of this error, if available
    #[must_use]
    pub fn span(&self) -> Option<(u32, u32)> {
        match self {
            ValidationError::UnbalancedStructure { opening_span, .. } => {
                Some((opening_span.start, opening_span.length))
            }
            ValidationError::UnclosedTag { span, .. }
            | ValidationError::OrphanedTag { span, .. }
            | ValidationError::UnmatchedBlockName { span, .. }
            | ValidationError::MissingRequiredArguments { span, .. }
            | ValidationError::TooManyArguments { span, .. } => Some((span.start, span.length)),
        }
    }

    /// Get a diagnostic code string for this error type
    #[must_use]
    pub fn diagnostic_code(&self) -> &'static str {
        match self {
            ValidationError::UnclosedTag { .. } => "S100",
            ValidationError::UnbalancedStructure { .. } => "S101",
            ValidationError::OrphanedTag { .. } => "S102",
            ValidationError::UnmatchedBlockName { .. } => "S103",
            ValidationError::MissingRequiredArguments { .. } => "S104",
            ValidationError::TooManyArguments { .. } => "S105",
        }
    }
}

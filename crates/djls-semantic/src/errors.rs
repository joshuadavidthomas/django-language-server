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

    #[error("Tag '{tag}' requires '{{% load {library} %}}'")]
    UnloadedLibraryTag {
        tag: String,
        library: String,
        span: Span,
    },

    #[error("Tag '{tag}' requires one of: {}", libraries.iter().map(|l| format!("{{% load {l} %}}")).collect::<Vec<_>>().join(", "))]
    AmbiguousUnloadedTag {
        tag: String,
        /// All libraries that define this tag (for disambiguation)
        libraries: Vec<String>,
        span: Span,
    },

    #[error("Unknown filter '{filter}'")]
    UnknownFilter { filter: String, span: Span },

    #[error("Filter '{filter}' requires '{{% load {library} %}}'")]
    UnloadedLibraryFilter {
        filter: String,
        library: String,
        span: Span,
    },

    #[error("Filter '{filter}' requires one of: {}", libraries.iter().map(|l| format!("{{% load {l} %}}")).collect::<Vec<_>>().join(", "))]
    AmbiguousUnloadedFilter {
        filter: String,
        /// All libraries that define this filter (for disambiguation)
        libraries: Vec<String>,
        span: Span,
    },

    #[error("{message}")]
    ExtractedRuleViolation {
        tag: String,
        message: String,
        span: Span,
    },
}

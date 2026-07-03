use djls_source::Span;
use serde::Serialize;
use thiserror::Error;

#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize)]
pub enum ValidationError {
    #[error("Unclosed '{tag}' tag")]
    UnclosedTag { tag: String, span: Span },

    #[error("'{tag}' must be inside {context}")]
    OrphanedTag {
        tag: String,
        context: String,
        span: Span,
    },

    #[error("'{tag}' has no matching '{expected_opener}' block")]
    OrphanedClosingTag {
        tag: String,
        expected_opener: String,
        span: Span,
    },

    #[error("'{opening_tag}' block is not closed before '{expected_closing}'")]
    UnbalancedStructure {
        opening_tag: String,
        expected_closing: String,
        opening_span: Span,
        closing_span: Option<Span>,
    },

    #[error("Closing block '{got}' does not match opening block '{expected}'")]
    UnmatchedBlockName {
        expected: String,
        got: String,
        span: Span,
        opener_span: Span,
    },

    #[error("Unknown tag '{tag}'")]
    UnknownTag { tag: String, span: Span },

    #[error("Add '{app}' to INSTALLED_APPS to use tag '{tag}'")]
    TagNotInInstalledApps {
        tag: String,
        app: String,
        load_name: String,
        span: Span,
    },

    #[error("Tag '{tag}' requires the '{library}' tag library")]
    UnloadedTag {
        tag: String,
        library: String,
        span: Span,
    },

    #[error(
        "Tag '{tag}' is available from multiple libraries: {}",
        format_library_list(libraries)
    )]
    AmbiguousUnloadedTag {
        tag: String,
        libraries: Vec<String>,
        span: Span,
    },

    #[error("Unknown filter '{filter}'")]
    UnknownFilter { filter: String, span: Span },

    #[error("Add '{app}' to INSTALLED_APPS to use filter '{filter}'")]
    FilterNotInInstalledApps {
        filter: String,
        app: String,
        load_name: String,
        span: Span,
    },

    #[error("Filter '{filter}' requires the '{library}' tag library")]
    UnloadedFilter {
        filter: String,
        library: String,
        span: Span,
    },

    #[error(
        "Filter '{filter}' is available from multiple libraries: {}",
        format_library_list(libraries)
    )]
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

    #[error("Unknown template tag library '{name}'")]
    UnknownLibrary { name: String, span: Span },

    #[error("Add '{app}' to INSTALLED_APPS to use template tag library '{name}'")]
    LibraryNotInInstalledApps {
        name: String,
        app: String,
        candidates: Vec<String>,
        span: Span,
    },

    #[error("The 'extends' tag must be the first tag in the template")]
    ExtendsMustBeFirst { span: Span },

    #[error("The 'extends' tag can only appear once in a template")]
    MultipleExtends { span: Span },
}

fn format_library_list(libraries: &[String]) -> String {
    libraries
        .iter()
        .map(|library| format!("'{library}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

impl ValidationError {
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnclosedTag { .. } => "S100",
            Self::UnbalancedStructure { .. } | Self::OrphanedClosingTag { .. } => "S101",
            Self::OrphanedTag { .. } => "S102",
            Self::UnmatchedBlockName { .. } => "S103",
            Self::UnknownTag { .. } => "S108",
            Self::UnloadedTag { .. } => "S109",
            Self::AmbiguousUnloadedTag { .. } => "S110",
            Self::UnknownFilter { .. } => "S111",
            Self::UnloadedFilter { .. } => "S112",
            Self::AmbiguousUnloadedFilter { .. } => "S113",
            Self::ExpressionSyntaxError { .. } => "S114",
            Self::FilterMissingArgument { .. } => "S115",
            Self::FilterUnexpectedArgument { .. } => "S116",
            Self::ExtractedRuleViolation { .. } => "S117",
            Self::TagNotInInstalledApps { .. } => "S118",
            Self::FilterNotInInstalledApps { .. } => "S119",
            Self::UnknownLibrary { .. } => "S120",
            Self::LibraryNotInInstalledApps { .. } => "S121",
            Self::ExtendsMustBeFirst { .. } => "S122",
            Self::MultipleExtends { .. } => "S123",
        }
    }

    #[must_use]
    pub fn primary_span(&self) -> Option<Span> {
        match self {
            Self::UnbalancedStructure { opening_span, .. } => Some(*opening_span),
            Self::UnclosedTag { span, .. }
            | Self::OrphanedTag { span, .. }
            | Self::OrphanedClosingTag { span, .. }
            | Self::UnmatchedBlockName { span, .. }
            | Self::UnknownTag { span, .. }
            | Self::TagNotInInstalledApps { span, .. }
            | Self::UnloadedTag { span, .. }
            | Self::AmbiguousUnloadedTag { span, .. }
            | Self::UnknownFilter { span, .. }
            | Self::FilterNotInInstalledApps { span, .. }
            | Self::UnloadedFilter { span, .. }
            | Self::AmbiguousUnloadedFilter { span, .. }
            | Self::ExpressionSyntaxError { span, .. }
            | Self::FilterMissingArgument { span, .. }
            | Self::FilterUnexpectedArgument { span, .. }
            | Self::ExtractedRuleViolation { span, .. }
            | Self::UnknownLibrary { span, .. }
            | Self::LibraryNotInInstalledApps { span, .. }
            | Self::ExtendsMustBeFirst { span, .. }
            | Self::MultipleExtends { span, .. } => Some(*span),
        }
    }
}

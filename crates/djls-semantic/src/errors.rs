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

    #[error("'{got}' does not match '{expected}'")]
    UnmatchedBlockName {
        expected: String,
        got: String,
        span: Span,
    },

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

    #[error("Tag '{tag}' requires '{app}' in INSTALLED_APPS")]
    TagNotInInstalledApps {
        tag: String,
        app: String,
        load_name: String,
        span: Span,
    },

    #[error("Filter '{filter}' requires '{app}' in INSTALLED_APPS")]
    FilterNotInInstalledApps {
        filter: String,
        app: String,
        load_name: String,
        span: Span,
    },

    #[error("Unknown template tag library '{name}'")]
    UnknownLibrary { name: String, span: Span },

    #[error("Template tag library '{name}' requires '{app}' in INSTALLED_APPS")]
    LibraryNotInInstalledApps {
        name: String,
        app: String,
        candidates: Vec<String>,
        span: Span,
    },

    #[error("'{{% extends %}}' must be the first tag in the template")]
    ExtendsMustBeFirst { span: Span },

    #[error("'{{% extends %}}' cannot appear more than once in the same template")]
    MultipleExtends { span: Span },
}

impl ValidationError {
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::UnclosedTag { .. } => "unclosed-tag",
            Self::UnbalancedStructure { .. } => "unbalanced-structure",
            Self::OrphanedTag { .. } => "orphaned-tag",
            Self::UnmatchedBlockName { .. } => "unmatched-block-name",
            Self::UnknownTag { .. } => "unknown-tag",
            Self::UnloadedTag { .. } => "unloaded-tag",
            Self::AmbiguousUnloadedTag { .. } => "ambiguous-unloaded-tag",
            Self::UnknownFilter { .. } => "unknown-filter",
            Self::UnloadedFilter { .. } => "unloaded-filter",
            Self::AmbiguousUnloadedFilter { .. } => "ambiguous-unloaded-filter",
            Self::ExpressionSyntaxError { .. } => "expression-syntax-error",
            Self::FilterMissingArgument { .. } => "filter-missing-argument",
            Self::FilterUnexpectedArgument { .. } => "filter-unexpected-argument",
            Self::ExtractedRuleViolation { .. } => "extracted-rule-violation",
            Self::TagNotInInstalledApps { .. } => "tag-not-in-installed-apps",
            Self::FilterNotInInstalledApps { .. } => "filter-not-in-installed-apps",
            Self::UnknownLibrary { .. } => "unknown-library",
            Self::LibraryNotInInstalledApps { .. } => "library-not-in-installed-apps",
            Self::ExtendsMustBeFirst { .. } => "extends-must-be-first",
            Self::MultipleExtends { .. } => "multiple-extends",
        }
    }

    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnclosedTag { .. } => "S100",
            Self::UnbalancedStructure { .. } => "S101",
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
            | Self::UnmatchedBlockName { span, .. }
            | Self::UnknownTag { span, .. }
            | Self::UnloadedTag { span, .. }
            | Self::AmbiguousUnloadedTag { span, .. }
            | Self::UnknownFilter { span, .. }
            | Self::UnloadedFilter { span, .. }
            | Self::AmbiguousUnloadedFilter { span, .. }
            | Self::ExpressionSyntaxError { span, .. }
            | Self::FilterMissingArgument { span, .. }
            | Self::FilterUnexpectedArgument { span, .. }
            | Self::ExtractedRuleViolation { span, .. }
            | Self::TagNotInInstalledApps { span, .. }
            | Self::FilterNotInInstalledApps { span, .. }
            | Self::UnknownLibrary { span, .. }
            | Self::LibraryNotInInstalledApps { span, .. }
            | Self::ExtendsMustBeFirst { span, .. }
            | Self::MultipleExtends { span, .. } => Some(*span),
        }
    }
}

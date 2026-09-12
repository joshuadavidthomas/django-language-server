use djls_project::UnreadRegistration;
use djls_project::UnreadShape;
use djls_source::File;
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
        got_span: Span,
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

    /// A loaded library has registrations DJLS could not read; unrecognized
    /// tags and filters from that library are not reported.
    #[error(
        "{}",
        unreadable_library_message(
            library,
            file_name,
            *first_line,
            *first_shape,
            unread.len()
        )
    )]
    UnreadableLibrary {
        /// Load name as written in the Template.
        library: String,
        /// Span of that load argument.
        span: Span,
        /// The Template Library's Python file.
        ///
        /// This is not named `source` because `thiserror` reserves that field
        /// name for an underlying error.
        #[serde(skip)]
        registration_file: File,
        /// Basename of the Template Library's Python file.
        file_name: String,
        /// One-based line of the first unread statement.
        first_line: u32,
        /// Shape of the first unread statement.
        first_shape: UnreadShape,
        /// Every unread statement in the Template Library.
        unread: Vec<UnreadRegistration>,
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

fn unreadable_library_message(
    library: &str,
    file_name: &str,
    first_line: u32,
    first_shape: UnreadShape,
    unread_count: usize,
) -> String {
    if unread_count == 1 {
        format!(
            "DJLS could not read a registration in `{file_name}` at line {first_line} ({first_shape}), so unrecognized tags and filters from `{library}` are not reported"
        )
    } else {
        format!(
            "DJLS could not read {unread_count} registrations in `{file_name}` (first at line {first_line}: {first_shape}), so unrecognized tags and filters from `{library}` are not reported"
        )
    }
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
            Self::UnreadableLibrary { .. } => "S124",
        }
    }

    pub(crate) fn unreadable_library(
        db: &dyn crate::Db,
        library: String,
        span: Span,
        registration_file: File,
        unread: Vec<UnreadRegistration>,
    ) -> Option<Self> {
        let first = unread.first()?;
        let file_name = registration_file
            .path(db)
            .file_name()
            .unwrap_or_else(|| registration_file.path(db).as_str())
            .to_string();
        let (line, _) = registration_file
            .line_index(db)
            .to_line_col(first.span.start_offset())
            .into();
        Some(Self::UnreadableLibrary {
            library,
            span,
            registration_file,
            file_name,
            first_line: line.saturating_add(1),
            first_shape: first.shape,
            unread,
        })
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
            | Self::UnreadableLibrary { span, .. }
            | Self::ExtendsMustBeFirst { span, .. }
            | Self::MultipleExtends { span, .. } => Some(*span),
        }
    }
}

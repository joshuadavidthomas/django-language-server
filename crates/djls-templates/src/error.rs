use serde::Serialize;
use thiserror::Error;
use tower_lsp_server::lsp_types::DiagnosticSeverity;

use crate::ast::NodeListError;
use crate::ast::Span;
use crate::lexer::LexerError;
use crate::parser::ParseError;
use crate::syntax::StructuralError;
use crate::validation::SemanticError;

/// Unified diagnostic type for all template processing stages
#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize)]
pub enum TemplateDiagnostic {
    #[error("Lexical error: {0}")]
    Lexical(#[from] LexerError),

    #[error("Parse error: {0}")]
    Parse(#[from] ParseError),

    #[error("Structural error: {0}")]
    Structural(#[from] StructuralError),

    #[error("Semantic error: {0}")]
    Semantic(#[from] SemanticError),

    // Legacy - to be migrated
    #[error("Validation error: {0}")]
    NodeList(#[from] NodeListError),
}

impl TemplateDiagnostic {
    /// Get the severity level of this diagnostic
    #[must_use]
    pub fn severity(&self) -> DiagnosticSeverity {
        match self {
            Self::Lexical(_) | Self::Parse(_) | Self::Structural(_) => DiagnosticSeverity::ERROR,
            Self::Semantic(_) => DiagnosticSeverity::WARNING,
            Self::NodeList(err) => {
                // Map NodeListError variants to severities
                match err {
                    NodeListError::EmptyNodeList => DiagnosticSeverity::INFORMATION,
                    _ => DiagnosticSeverity::ERROR,
                }
            }
        }
    }

    /// Get a diagnostic code for this error
    #[must_use]
    pub fn code(&self) -> String {
        match self {
            Self::Lexical(_) => "E001".to_string(),
            Self::Parse(_) => "E002".to_string(),
            Self::Structural(_) => "E003".to_string(),
            Self::Semantic(_) => "W001".to_string(),
            Self::NodeList(err) => err.diagnostic_code().to_string(),
        }
    }

    /// Get the display message for this diagnostic
    #[must_use]
    pub fn message(&self) -> String {
        self.to_string()
    }

    /// Get the span where this diagnostic occurred, if available
    #[must_use]
    pub fn span(&self) -> Option<Span> {
        match self {
            Self::Structural(err) => match err {
                StructuralError::UnclosedBlock { opener_span, .. } => Some(*opener_span),
                StructuralError::OrphanedIntermediate { span, .. } => Some(*span),
                StructuralError::MismatchedCloser { closer_span, .. } => Some(*closer_span),
                StructuralError::UnexpectedCloser { span, .. } => Some(*span),
                StructuralError::DuplicateBranch { span, .. } => Some(*span),
            },
            Self::Semantic(err) => match err {
                SemanticError::MissingRequiredArg { span, .. }
                | SemanticError::InvalidArgType { span, .. }
                | SemanticError::UnknownArgument { span, .. }
                | SemanticError::InvalidChoiceValue { span, .. }
                | SemanticError::TooManyPositionalArgs { span, .. }
                | SemanticError::ConflictingArgs { span, .. } => Some(*span),
            },
            Self::NodeList(err) => err.span().map(|(start, length)| Span { start, length }),
            _ => None,
        }
    }

    /// Get related information for this diagnostic
    #[must_use]
    pub fn related_info(&self) -> Vec<RelatedInfo> {
        match self {
            Self::Structural(StructuralError::MismatchedCloser {
                opener_span,
                expected,
                ..
            }) => vec![RelatedInfo {
                span: *opener_span,
                message: format!("Opening tag expects '{expected}'"),
            }],
            Self::Structural(StructuralError::DuplicateBranch { first_span, .. }) => {
                vec![RelatedInfo {
                    span: *first_span,
                    message: "First occurrence here".to_string(),
                }]
            }
            _ => vec![],
        }
    }
}

/// Related diagnostic information
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RelatedInfo {
    pub span: Span,
    pub message: String,
}

// Keep TemplateError as alias for backwards compatibility
// TODO: Remove after migration
pub type TemplateError = TemplateDiagnostic;

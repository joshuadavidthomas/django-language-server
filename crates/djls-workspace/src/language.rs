//! Language identification for document routing
//!
//! Maps LSP language identifiers to internal [`FileKind`] for analyzer routing.
//! Language IDs come from the LSP client and determine how files are processed.

use djls_source::FileKind;

/// Language identifier as reported by the LSP client.
///
/// These identifiers follow VS Code's language ID conventions and determine
/// which analyzers and features are available for a document. Converts to
/// [`FileKind`] to route files to appropriate analyzers (Python vs Template).
#[derive(Clone, Debug, PartialEq)]
pub enum LanguageId {
    Html,
    HtmlDjango,
    Other,
    PlainText,
    Python,
}

impl From<&str> for LanguageId {
    fn from(language_id: &str) -> Self {
        match language_id {
            "django-html" | "htmldjango" => Self::HtmlDjango,
            "html" => Self::Html,
            "plaintext" => Self::PlainText,
            "python" => Self::Python,
            _ => Self::Other,
        }
    }
}

impl From<String> for LanguageId {
    fn from(language_id: String) -> Self {
        Self::from(language_id.as_str())
    }
}

impl From<LanguageId> for FileKind {
    fn from(language_id: LanguageId) -> Self {
        match language_id {
            LanguageId::Python => Self::Python,
            LanguageId::HtmlDjango => Self::Template,
            LanguageId::Html | LanguageId::PlainText | LanguageId::Other => Self::Other,
        }
    }
}

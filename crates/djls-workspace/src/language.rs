use crate::FileKind;

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

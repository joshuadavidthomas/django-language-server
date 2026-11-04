use compact_str::CompactString;
use djls_source::Span;
use serde::Serialize;

use crate::quotes::TemplateString;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct TagBit {
    pub(crate) text: CompactString,
    pub span: Span,
}

impl TagBit {
    #[must_use]
    pub fn new(text: impl Into<CompactString>, span: Span) -> Self {
        Self {
            text: text.into(),
            span,
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub fn template_string(&self) -> TemplateString<'_> {
        TemplateString::parse(&self.text, self.span)
    }
}

impl AsRef<str> for TagBit {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct FilterArgument {
    pub(crate) text: CompactString,
    pub(crate) span: Span,
}

impl FilterArgument {
    #[must_use]
    pub fn new(text: impl Into<CompactString>, span: Span) -> Self {
        Self {
            text: text.into(),
            span,
        }
    }

    #[must_use]
    pub(crate) fn as_str(&self) -> &str {
        &self.text
    }
}

impl AsRef<str> for FilterArgument {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

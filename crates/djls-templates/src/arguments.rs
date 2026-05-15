use djls_source::Span;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum Quote {
    Single,
    Double,
}

impl Quote {
    fn from_delimiter(ch: char) -> Option<Self> {
        match ch {
            '\'' => Some(Self::Single),
            '"' => Some(Self::Double),
            _ => None,
        }
    }

    #[must_use]
    pub fn delimiter(self) -> char {
        match self {
            Self::Single => '\'',
            Self::Double => '"',
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TemplateString<'a> {
    Quoted(QuotedTemplateString<'a>),
    Unquoted(&'a str),
}

impl<'a> TemplateString<'a> {
    #[must_use]
    pub fn parse(raw: &'a str) -> Self {
        let raw = raw.trim();
        let Some(first) = raw.chars().next() else {
            return Self::Unquoted(raw);
        };
        let Some(quote) = Quote::from_delimiter(first) else {
            return Self::Unquoted(raw);
        };

        if raw.len() < 2 || !raw.ends_with(quote.delimiter()) {
            return Self::Unquoted(raw);
        }

        Self::Quoted(QuotedTemplateString {
            raw,
            value: &raw[1..raw.len() - 1],
            quote,
        })
    }

    #[must_use]
    pub fn raw(self) -> &'a str {
        match self {
            Self::Quoted(value) => value.raw,
            Self::Unquoted(raw) => raw,
        }
    }

    #[must_use]
    pub fn value(self) -> &'a str {
        match self {
            Self::Quoted(value) => value.value,
            Self::Unquoted(raw) => raw,
        }
    }

    #[must_use]
    pub fn quoted_value(self) -> Option<&'a str> {
        match self {
            Self::Quoted(value) => Some(value.value),
            Self::Unquoted(_) => None,
        }
    }

    #[must_use]
    pub fn quote(self) -> Option<Quote> {
        match self {
            Self::Quoted(value) => Some(value.quote),
            Self::Unquoted(_) => None,
        }
    }

    #[must_use]
    pub fn is_quoted(self) -> bool {
        matches!(self, Self::Quoted(_))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct QuotedTemplateString<'a> {
    raw: &'a str,
    value: &'a str,
    quote: Quote,
}

impl<'a> QuotedTemplateString<'a> {
    #[must_use]
    pub fn raw(self) -> &'a str {
        self.raw
    }

    #[must_use]
    pub fn value(self) -> &'a str {
        self.value
    }

    #[must_use]
    pub fn quote(self) -> Quote {
        self.quote
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct TagArgument {
    pub text: String,
    pub span: Span,
}

impl TagArgument {
    #[must_use]
    pub fn new(text: String, span: Span) -> Self {
        Self { text, span }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub fn template_string(&self) -> TemplateString<'_> {
        TemplateString::parse(&self.text)
    }
}

impl AsRef<str> for TagArgument {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct FilterArgument {
    pub text: String,
    pub span: Span,
}

impl FilterArgument {
    #[must_use]
    pub fn new(text: String, span: Span) -> Self {
        Self { text, span }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub fn template_string(&self) -> TemplateString<'_> {
        TemplateString::parse(&self.text)
    }
}

impl AsRef<str> for FilterArgument {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_string_recognizes_single_quoted_values() {
        let value = TemplateString::parse("'images/logo.png'");

        assert_eq!(value.raw(), "'images/logo.png'");
        assert_eq!(value.value(), "images/logo.png");
        assert_eq!(value.quote(), Some(Quote::Single));
    }

    #[test]
    fn template_string_recognizes_double_quoted_values() {
        let value = TemplateString::parse(r#""base.html""#);

        assert_eq!(value.raw(), r#""base.html""#);
        assert_eq!(value.value(), "base.html");
        assert_eq!(value.quote(), Some(Quote::Double));
    }

    #[test]
    fn template_string_leaves_bare_values_unquoted() {
        let value = TemplateString::parse("user.name");

        assert_eq!(value.raw(), "user.name");
        assert_eq!(value.value(), "user.name");
        assert_eq!(value.quote(), None);
    }
}

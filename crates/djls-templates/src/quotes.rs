use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum Quote {
    Single,
    Double,
}

impl TryFrom<char> for Quote {
    type Error = ();

    fn try_from(ch: char) -> Result<Self, Self::Error> {
        match ch {
            '\'' => Ok(Self::Single),
            '"' => Ok(Self::Double),
            _ => Err(()),
        }
    }
}

impl From<Quote> for char {
    fn from(quote: Quote) -> Self {
        match quote {
            Quote::Single => '\'',
            Quote::Double => '"',
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
        let Ok(quote) = Quote::try_from(first) else {
            return Self::Unquoted(raw);
        };

        let quote_char: char = quote.into();
        if raw.len() < 2 || !raw.ends_with(quote_char) {
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

/// Find positions of a delimiter character in `s`, skipping occurrences inside
/// single- or double-quoted regions.
///
/// When `handle_escapes` is true, `\` inside a quoted region escapes the next
/// character (so `\"` does not close the quote).
///
/// The callback receives the byte index of each unquoted delimiter found.
/// Return `true` from the callback to stop early.
pub(crate) fn for_each_unquoted(
    s: &str,
    delimiter: impl Fn(char) -> bool,
    handle_escapes: bool,
    mut cb: impl FnMut(usize) -> bool,
) {
    let mut quote: Option<char> = None;
    let mut escape = false;

    for (idx, ch) in s.char_indices() {
        if escape {
            escape = false;
            continue;
        }
        match ch {
            '\\' if handle_escapes && quote.is_some() => {
                escape = true;
            }
            '"' | '\'' if quote == Some(ch) => {
                quote = None;
            }
            '"' | '\'' if quote.is_none() => {
                quote = Some(ch);
            }
            _ if quote.is_some() => {}
            _ if delimiter(ch) && cb(idx) => {
                return;
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SplitPiece<'a> {
    pub text: &'a str,
    pub start: usize,
}

/// Split `s` on whitespace while respecting quoted regions (with escape handling).
///
/// Returns borrowed tokens with byte offsets relative to `s`.
pub(crate) fn split_on_whitespace_with_offsets(s: &str) -> Vec<SplitPiece<'_>> {
    let mut pieces = Vec::with_capacity((s.len() / 8).clamp(2, 8));
    let mut start = None;
    let mut quote: Option<char> = None;
    let mut escape = false;

    for (idx, ch) in s.char_indices() {
        if escape {
            escape = false;
            if start.is_none() {
                start = Some(idx.saturating_sub(1));
            }
            continue;
        }
        match ch {
            '\\' if quote.is_some() => {
                escape = true;
                if start.is_none() {
                    start = Some(idx);
                }
            }
            '"' | '\'' if quote == Some(ch) => {
                quote = None;
                if start.is_none() {
                    start = Some(idx);
                }
            }
            '"' | '\'' if quote.is_none() => {
                quote = Some(ch);
                if start.is_none() {
                    start = Some(idx);
                }
            }
            _ if quote.is_some() => {
                if start.is_none() {
                    start = Some(idx);
                }
            }
            _ if ch.is_whitespace() => {
                if let Some(s_start) = start.take() {
                    pieces.push(SplitPiece {
                        text: &s[s_start..idx],
                        start: s_start,
                    });
                }
            }
            _ => {
                if start.is_none() {
                    start = Some(idx);
                }
            }
        }
    }
    if let Some(s_start) = start {
        pieces.push(SplitPiece {
            text: &s[s_start..],
            start: s_start,
        });
    }
    pieces
}

/// Split `s` on whitespace while respecting quoted regions (with escape handling).
///
/// Returns owned strings for each whitespace-delimited token.
#[cfg(test)]
pub(crate) fn split_on_whitespace(s: &str) -> Vec<String> {
    split_on_whitespace_with_offsets(s)
        .into_iter()
        .map(|piece| piece.text.to_owned())
        .collect()
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

    #[test]
    fn unquoted_delimiters_found() {
        let mut positions = Vec::new();
        for_each_unquoted(
            "a|b|c",
            |ch| ch == '|',
            false,
            |idx| {
                positions.push(idx);
                false
            },
        );
        assert_eq!(positions, vec![1, 3]);
    }

    #[test]
    fn quoted_delimiters_skipped() {
        let mut positions = Vec::new();
        for_each_unquoted(
            "a|'b|c'|d",
            |ch| ch == '|',
            false,
            |idx| {
                positions.push(idx);
                false
            },
        );
        assert_eq!(positions, vec![1, 7]);
    }

    #[test]
    fn double_quotes() {
        let mut positions = Vec::new();
        for_each_unquoted(
            r#"a|"b|c"|d"#,
            |ch| ch == '|',
            false,
            |idx| {
                positions.push(idx);
                false
            },
        );
        assert_eq!(positions, vec![1, 7]);
    }

    #[test]
    fn escape_handling() {
        let mut positions = Vec::new();
        for_each_unquoted(
            r#""a\"b"|c"#,
            |ch| ch == '|',
            true,
            |idx| {
                positions.push(idx);
                false
            },
        );
        // The \" is escaped, so the quote doesn't close until the real "
        assert_eq!(positions, vec![6]);
    }

    #[test]
    fn escape_ignored_without_flag() {
        let mut positions = Vec::new();
        for_each_unquoted(
            r#""a\"b"|c"#,
            |ch| ch == '|',
            false,
            |idx| {
                positions.push(idx);
                false
            },
        );
        // Without escape handling, \" closes the quote, then b" opens a new one
        // "a\" -> quote closed at \", then b" opens, |c is outside... actually:
        // char-by-char: " opens, a inside, \ inside, " closes, b outside, " opens, | inside, c inside
        assert!(positions.is_empty());
    }

    #[test]
    fn early_stop() {
        let mut positions = Vec::new();
        for_each_unquoted(
            "a|b|c|d",
            |ch| ch == '|',
            false,
            |idx| {
                positions.push(idx);
                positions.len() >= 2
            },
        );
        assert_eq!(positions, vec![1, 3]);
    }

    #[test]
    fn split_whitespace_simple() {
        assert_eq!(
            split_on_whitespace("load i18n l10n"),
            vec!["load", "i18n", "l10n"]
        );
    }

    #[test]
    fn split_whitespace_quoted() {
        assert_eq!(
            split_on_whitespace(r#"if x == "hello world""#),
            vec!["if", "x", "==", r#""hello world""#]
        );
    }

    #[test]
    fn split_whitespace_escaped() {
        assert_eq!(
            split_on_whitespace(r#"blocktrans "it\"s fine""#),
            vec!["blocktrans", r#""it\"s fine""#]
        );
    }

    #[test]
    fn split_whitespace_empty() {
        assert!(split_on_whitespace("").is_empty());
        assert!(split_on_whitespace("   ").is_empty());
    }

    #[test]
    fn split_whitespace_offsets() {
        let pieces = split_on_whitespace_with_offsets(r#"  url "view name" as view"#);

        assert_eq!(
            pieces,
            vec![
                SplitPiece {
                    text: "url",
                    start: 2,
                },
                SplitPiece {
                    text: r#""view name""#,
                    start: 6,
                },
                SplitPiece {
                    text: "as",
                    start: 18,
                },
                SplitPiece {
                    text: "view",
                    start: 21,
                },
            ]
        );
    }
}

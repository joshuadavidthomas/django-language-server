#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TemplateString<'a> {
    Quoted(&'a str),
    Unquoted(&'a str),
}

impl<'a> TemplateString<'a> {
    #[must_use]
    pub(crate) fn parse(raw: &'a str) -> Self {
        let raw = raw.trim();
        let Some(first) = raw.chars().next() else {
            return Self::Unquoted(raw);
        };
        if first != '\'' && first != '"' {
            return Self::Unquoted(raw);
        }

        if raw.len() < 2 || !raw.ends_with(first) {
            return Self::Unquoted(raw);
        }

        Self::Quoted(&raw[1..raw.len() - 1])
    }

    #[must_use]
    pub fn value(self) -> &'a str {
        match self {
            Self::Quoted(value) | Self::Unquoted(value) => value,
        }
    }

    #[must_use]
    pub fn quoted_value(self) -> Option<&'a str> {
        match self {
            Self::Quoted(value) => Some(value),
            Self::Unquoted(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QuoteEscapeMode {
    LiteralBackslash,
    BackslashEscapesNext,
}

impl QuoteEscapeMode {
    fn treats_backslash_as_escape(self) -> bool {
        matches!(self, Self::BackslashEscapesNext)
    }
}

struct UnquotedDelimiterIndices<'a> {
    chars: std::str::CharIndices<'a>,
    delimiter: char,
    escape_mode: QuoteEscapeMode,
    quote: Option<char>,
    escape: bool,
}

impl<'a> UnquotedDelimiterIndices<'a> {
    fn new(s: &'a str, delimiter: char, escape_mode: QuoteEscapeMode) -> Self {
        Self {
            chars: s.char_indices(),
            delimiter,
            escape_mode,
            quote: None,
            escape: false,
        }
    }
}

impl Iterator for UnquotedDelimiterIndices<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        for (idx, ch) in self.chars.by_ref() {
            if self.escape {
                self.escape = false;
                continue;
            }
            match ch {
                '\\' if self.escape_mode.treats_backslash_as_escape() && self.quote.is_some() => {
                    self.escape = true;
                }
                '"' | '\'' if self.quote == Some(ch) => {
                    self.quote = None;
                }
                '"' | '\'' if self.quote.is_none() => {
                    self.quote = Some(ch);
                }
                _ if self.quote.is_some() => {}
                _ if ch == self.delimiter => return Some(idx),
                _ => {}
            }
        }
        None
    }
}

/// Return the byte index of the first delimiter outside quoted regions.
#[must_use]
pub(crate) fn first_unquoted_delimiter_index(
    s: &str,
    delimiter: char,
    escape_mode: QuoteEscapeMode,
) -> Option<usize> {
    UnquotedDelimiterIndices::new(s, delimiter, escape_mode).next()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SplitPiece<'a> {
    pub text: &'a str,
    pub start: usize,
}

/// Split `s` on a delimiter while respecting quoted regions.
///
/// Returns borrowed pieces with byte offsets relative to `s`.
pub(crate) fn split_on_unquoted_delimiter_with_offsets(
    s: &str,
    delimiter: char,
    escape_mode: QuoteEscapeMode,
) -> Vec<SplitPiece<'_>> {
    let mut pieces = Vec::with_capacity((s.len() / 8).clamp(2, 8));
    let mut start = 0;

    for idx in UnquotedDelimiterIndices::new(s, delimiter, escape_mode) {
        pieces.push(SplitPiece {
            text: &s[start..idx],
            start,
        });
        start = idx + delimiter.len_utf8();
    }

    pieces.push(SplitPiece {
        text: &s[start..],
        start,
    });
    pieces
}

/// Split `s` on whitespace while respecting quoted regions (with escape handling).
///
/// Returns borrowed tokens with byte offsets relative to `s`.
pub(crate) fn split_on_whitespace_with_offsets(s: &str) -> Vec<SplitPiece<'_>> {
    let mut pieces = Vec::with_capacity((s.len() / 8).clamp(2, 8));
    let mut start = None;
    let mut quote: Option<char> = None;
    let mut escape = false;
    let escape_mode = QuoteEscapeMode::BackslashEscapesNext;

    for (idx, ch) in s.char_indices() {
        if escape {
            escape = false;
            if start.is_none() {
                start = Some(idx.saturating_sub(1));
            }
            continue;
        }
        match ch {
            '\\' if escape_mode.treats_backslash_as_escape() && quote.is_some() => {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn split_on_whitespace(s: &str) -> Vec<String> {
        split_on_whitespace_with_offsets(s)
            .into_iter()
            .map(|piece| piece.text.to_owned())
            .collect()
    }

    #[test]
    fn template_string_recognizes_single_quoted_values() {
        let value = TemplateString::parse("'images/logo.png'");

        assert_eq!(value, TemplateString::Quoted("images/logo.png"));
        assert_eq!(value.value(), "images/logo.png");
        assert_eq!(value.quoted_value(), Some("images/logo.png"));
    }

    #[test]
    fn template_string_recognizes_double_quoted_values() {
        let value = TemplateString::parse(r#""base.html""#);

        assert_eq!(value, TemplateString::Quoted("base.html"));
        assert_eq!(value.value(), "base.html");
        assert_eq!(value.quoted_value(), Some("base.html"));
    }

    #[test]
    fn template_string_leaves_bare_values_unquoted() {
        let value = TemplateString::parse("user.name");

        assert_eq!(value, TemplateString::Unquoted("user.name"));
        assert_eq!(value.value(), "user.name");
        assert_eq!(value.quoted_value(), None);
    }

    #[test]
    fn first_unquoted_delimiter_index_returns_first_delimiter() {
        assert_eq!(
            first_unquoted_delimiter_index("a|b|c", '|', QuoteEscapeMode::LiteralBackslash),
            Some(1)
        );
    }

    #[test]
    fn split_unquoted_delimiters() {
        let pieces = split_on_unquoted_delimiter_with_offsets(
            "a|b|c",
            '|',
            QuoteEscapeMode::LiteralBackslash,
        );
        assert_eq!(
            pieces,
            vec![
                SplitPiece {
                    text: "a",
                    start: 0,
                },
                SplitPiece {
                    text: "b",
                    start: 2,
                },
                SplitPiece {
                    text: "c",
                    start: 4,
                },
            ]
        );
    }

    #[test]
    fn quoted_delimiters_skipped() {
        let pieces = split_on_unquoted_delimiter_with_offsets(
            "a|'b|c'|d",
            '|',
            QuoteEscapeMode::LiteralBackslash,
        );
        assert_eq!(
            pieces,
            vec![
                SplitPiece {
                    text: "a",
                    start: 0,
                },
                SplitPiece {
                    text: "'b|c'",
                    start: 2,
                },
                SplitPiece {
                    text: "d",
                    start: 8,
                },
            ]
        );
    }

    #[test]
    fn double_quotes() {
        let pieces = split_on_unquoted_delimiter_with_offsets(
            r#"a|"b|c"|d"#,
            '|',
            QuoteEscapeMode::LiteralBackslash,
        );
        assert_eq!(
            pieces,
            vec![
                SplitPiece {
                    text: "a",
                    start: 0,
                },
                SplitPiece {
                    text: r#""b|c""#,
                    start: 2,
                },
                SplitPiece {
                    text: "d",
                    start: 8,
                },
            ]
        );
    }

    #[test]
    fn escape_handling() {
        let pieces = split_on_unquoted_delimiter_with_offsets(
            r#""a\"b"|c"#,
            '|',
            QuoteEscapeMode::BackslashEscapesNext,
        );

        // The \" is escaped, so the quote doesn't close until the real "
        assert_eq!(
            pieces,
            vec![
                SplitPiece {
                    text: r#""a\"b""#,
                    start: 0,
                },
                SplitPiece {
                    text: "c",
                    start: 7,
                },
            ]
        );
    }

    #[test]
    fn escape_ignored_when_backslash_is_literal() {
        let pieces = split_on_unquoted_delimiter_with_offsets(
            r#""a\"b"|c"#,
            '|',
            QuoteEscapeMode::LiteralBackslash,
        );

        // With literal backslashes, \" closes the quote, then b" opens a new one.
        assert_eq!(
            pieces,
            vec![SplitPiece {
                text: r#""a\"b"|c"#,
                start: 0,
            }]
        );
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

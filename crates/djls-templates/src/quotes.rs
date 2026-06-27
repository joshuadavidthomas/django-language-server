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
enum QuoteScanState {
    Outside,
    Inside(char),
    AfterBackslash(char),
}

impl QuoteScanState {
    fn is_outside_quote(self) -> bool {
        matches!(self, Self::Outside)
    }

    fn after_literal_backslash(self, ch: char) -> Self {
        match self {
            Self::Outside if matches!(ch, '"' | '\'') => Self::Inside(ch),
            Self::Outside => Self::Outside,
            Self::Inside(quote) if ch == quote => Self::Outside,
            Self::Inside(quote) | Self::AfterBackslash(quote) => Self::Inside(quote),
        }
    }

    fn after_escaping_backslash(self, ch: char) -> Self {
        match self {
            Self::Outside if matches!(ch, '"' | '\'') => Self::Inside(ch),
            Self::Outside => Self::Outside,
            Self::Inside(quote) if ch == '\\' => Self::AfterBackslash(quote),
            Self::Inside(quote) if ch == quote => Self::Outside,
            Self::Inside(quote) | Self::AfterBackslash(quote) => Self::Inside(quote),
        }
    }
}

struct UnquotedDelimiterIndices<'a> {
    chars: std::str::CharIndices<'a>,
    delimiter: char,
    state: QuoteScanState,
}

impl<'a> UnquotedDelimiterIndices<'a> {
    fn new(s: &'a str, delimiter: char) -> Self {
        Self {
            chars: s.char_indices(),
            delimiter,
            state: QuoteScanState::Outside,
        }
    }
}

impl Iterator for UnquotedDelimiterIndices<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        for (idx, ch) in self.chars.by_ref() {
            if self.state.is_outside_quote() && ch == self.delimiter {
                return Some(idx);
            }
            self.state = self.state.after_literal_backslash(ch);
        }
        None
    }
}

/// Return the byte index of the first delimiter outside quoted regions.
#[must_use]
pub(crate) fn first_unquoted_delimiter_index(s: &str, delimiter: char) -> Option<usize> {
    UnquotedDelimiterIndices::new(s, delimiter).next()
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
) -> Vec<SplitPiece<'_>> {
    let mut pieces = Vec::with_capacity((s.len() / 8).clamp(2, 8));
    let mut start = 0;

    for idx in UnquotedDelimiterIndices::new(s, delimiter) {
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
    let mut state = QuoteScanState::Outside;

    for (idx, ch) in s.char_indices() {
        match state {
            QuoteScanState::AfterBackslash(quote) => {
                state = QuoteScanState::Inside(quote);
                if start.is_none() {
                    start = Some(idx.saturating_sub(1));
                }
            }
            QuoteScanState::Inside(_) => {
                state = state.after_escaping_backslash(ch);
                if start.is_none() {
                    start = Some(idx);
                }
            }
            QuoteScanState::Outside => match ch {
                '"' | '\'' => {
                    state = state.after_escaping_backslash(ch);
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
            },
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
        assert_eq!(first_unquoted_delimiter_index("a|b|c", '|'), Some(1));
    }

    #[test]
    fn split_unquoted_delimiters() {
        let pieces = split_on_unquoted_delimiter_with_offsets("a|b|c", '|');
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
        let pieces = split_on_unquoted_delimiter_with_offsets("a|'b|c'|d", '|');
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
        let pieces = split_on_unquoted_delimiter_with_offsets(r#"a|"b|c"|d"#, '|');
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
    fn escape_ignored_when_backslash_is_literal() {
        let pieces = split_on_unquoted_delimiter_with_offsets(r#""a\"b"|c"#, '|');

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

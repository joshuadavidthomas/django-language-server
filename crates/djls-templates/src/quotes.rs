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
enum State {
    Outside,
    Quoted(char),
    Escaped(char),
}

impl State {
    fn is_outside(self) -> bool {
        matches!(self, Self::Outside)
    }

    fn feed(self, ch: char) -> Self {
        match self {
            Self::Outside if matches!(ch, '"' | '\'') => Self::Quoted(ch),
            Self::Outside => Self::Outside,
            Self::Quoted(quote) if ch == quote => Self::Outside,
            Self::Quoted(quote) | Self::Escaped(quote) => Self::Quoted(quote),
        }
    }

    fn feed_escaped(self, ch: char) -> Self {
        match self {
            Self::Outside if matches!(ch, '"' | '\'') => Self::Quoted(ch),
            Self::Outside => Self::Outside,
            Self::Quoted(quote) if ch == '\\' => Self::Escaped(quote),
            Self::Quoted(quote) if ch == quote => Self::Outside,
            Self::Quoted(quote) | Self::Escaped(quote) => Self::Quoted(quote),
        }
    }
}

pub(crate) struct Delimiters<'a> {
    chars: std::str::CharIndices<'a>,
    delimiter: char,
    state: State,
}

impl<'a> Delimiters<'a> {
    pub(crate) fn new(input: &'a str, delimiter: char) -> Self {
        Self {
            chars: input.char_indices(),
            delimiter,
            state: State::Outside,
        }
    }
}

impl Iterator for Delimiters<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        for (byte_index, ch) in self.chars.by_ref() {
            if self.state.is_outside() && ch == self.delimiter {
                return Some(byte_index);
            }
            self.state = self.state.feed(ch);
        }
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Segment<'a> {
    pub text: &'a str,
    pub start_byte: usize,
}

/// Split `input` on a delimiter while respecting quoted regions.
///
/// Returns borrowed segments with byte offsets relative to `input`.
pub(crate) fn split_on_unquoted_delimiter_with_offsets(
    input: &str,
    delimiter: char,
) -> Vec<Segment<'_>> {
    let mut segments = Vec::with_capacity((input.len() / 8).clamp(2, 8));
    let mut start_byte = 0;

    for delimiter_byte in Delimiters::new(input, delimiter) {
        segments.push(Segment {
            text: &input[start_byte..delimiter_byte],
            start_byte,
        });
        start_byte = delimiter_byte + delimiter.len_utf8();
    }

    segments.push(Segment {
        text: &input[start_byte..],
        start_byte,
    });
    segments
}

/// Split `input` on whitespace while respecting quoted regions (with escape handling).
///
/// Returns borrowed tokens with byte offsets relative to `input`.
pub(crate) fn split_on_whitespace_with_offsets(input: &str) -> Vec<Segment<'_>> {
    let mut segments = Vec::with_capacity((input.len() / 8).clamp(2, 8));
    let mut start_byte = None;
    let mut state = State::Outside;

    for (byte_index, ch) in input.char_indices() {
        match state {
            State::Escaped(quote) => {
                state = State::Quoted(quote);
                if start_byte.is_none() {
                    start_byte = Some(byte_index.saturating_sub(1));
                }
            }
            State::Quoted(_) => {
                state = state.feed_escaped(ch);
                if start_byte.is_none() {
                    start_byte = Some(byte_index);
                }
            }
            State::Outside => match ch {
                '"' | '\'' => {
                    state = state.feed_escaped(ch);
                    if start_byte.is_none() {
                        start_byte = Some(byte_index);
                    }
                }
                _ if ch.is_whitespace() => {
                    if let Some(segment_start_byte) = start_byte.take() {
                        segments.push(Segment {
                            text: &input[segment_start_byte..byte_index],
                            start_byte: segment_start_byte,
                        });
                    }
                }
                _ => {
                    if start_byte.is_none() {
                        start_byte = Some(byte_index);
                    }
                }
            },
        }
    }
    if let Some(segment_start_byte) = start_byte {
        segments.push(Segment {
            text: &input[segment_start_byte..],
            start_byte: segment_start_byte,
        });
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split_on_whitespace(input: &str) -> Vec<String> {
        split_on_whitespace_with_offsets(input)
            .into_iter()
            .map(|segment| segment.text.to_owned())
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
    fn delimiters_returns_first_delimiter() {
        assert_eq!(Delimiters::new("a|b|c", '|').next(), Some(1));
    }

    #[test]
    fn split_unquoted_delimiters() {
        let segments = split_on_unquoted_delimiter_with_offsets("a|b|c", '|');
        assert_eq!(
            segments,
            vec![
                Segment {
                    text: "a",
                    start_byte: 0,
                },
                Segment {
                    text: "b",
                    start_byte: 2,
                },
                Segment {
                    text: "c",
                    start_byte: 4,
                },
            ]
        );
    }

    #[test]
    fn quoted_delimiters_skipped() {
        let segments = split_on_unquoted_delimiter_with_offsets("a|'b|c'|d", '|');
        assert_eq!(
            segments,
            vec![
                Segment {
                    text: "a",
                    start_byte: 0,
                },
                Segment {
                    text: "'b|c'",
                    start_byte: 2,
                },
                Segment {
                    text: "d",
                    start_byte: 8,
                },
            ]
        );
    }

    #[test]
    fn double_quotes() {
        let segments = split_on_unquoted_delimiter_with_offsets(r#"a|"b|c"|d"#, '|');
        assert_eq!(
            segments,
            vec![
                Segment {
                    text: "a",
                    start_byte: 0,
                },
                Segment {
                    text: r#""b|c""#,
                    start_byte: 2,
                },
                Segment {
                    text: "d",
                    start_byte: 8,
                },
            ]
        );
    }

    #[test]
    fn escape_ignored_when_backslash_is_literal() {
        let segments = split_on_unquoted_delimiter_with_offsets(r#""a\"b"|c"#, '|');

        // With literal backslashes, \" closes the quote, then b" opens a new one.
        assert_eq!(
            segments,
            vec![Segment {
                text: r#""a\"b"|c"#,
                start_byte: 0,
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
        let segments = split_on_whitespace_with_offsets(r#"  url "view name" as view"#);

        assert_eq!(
            segments,
            vec![
                Segment {
                    text: "url",
                    start_byte: 2,
                },
                Segment {
                    text: r#""view name""#,
                    start_byte: 6,
                },
                Segment {
                    text: "as",
                    start_byte: 18,
                },
                Segment {
                    text: "view",
                    start_byte: 21,
                },
            ]
        );
    }
}

use djls_source::Span;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TemplateString<'a> {
    Quoted { value: &'a str, span: Span },
    Unquoted(&'a str),
}

impl<'a> TemplateString<'a> {
    #[must_use]
    pub(crate) fn parse(raw: &'a str, raw_span: Span) -> Self {
        let trim_start_len = raw.len() - raw.trim_start().len();
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

        let value = &raw[1..raw.len() - 1];
        let value_start = raw_span
            .start_usize()
            .saturating_add(trim_start_len)
            .saturating_add(first.len_utf8());
        let span = Span::saturating_from_parts_usize(value_start, value.len());

        Self::Quoted { value, span }
    }
}

// Quote, escape, and delimiter characters are all ASCII, so scanning bytes is
// safe: UTF-8 continuation bytes can never match them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Outside,
    Quoted(u8),
    Escaped(u8),
}

impl State {
    fn is_outside(self) -> bool {
        matches!(self, Self::Outside)
    }

    fn feed(self, byte: u8) -> Self {
        match self {
            Self::Outside if matches!(byte, b'"' | b'\'') => Self::Quoted(byte),
            Self::Outside => Self::Outside,
            Self::Quoted(quote) if byte == quote => Self::Outside,
            Self::Quoted(quote) | Self::Escaped(quote) => Self::Quoted(quote),
        }
    }

    fn feed_escaped(self, byte: u8) -> Self {
        match self {
            Self::Outside if matches!(byte, b'"' | b'\'') => Self::Quoted(byte),
            Self::Outside => Self::Outside,
            Self::Quoted(quote) if byte == b'\\' => Self::Escaped(quote),
            Self::Quoted(quote) if byte == quote => Self::Outside,
            Self::Quoted(quote) | Self::Escaped(quote) => Self::Quoted(quote),
        }
    }
}

struct DelimiterIndices<'a> {
    input: &'a str,
    cursor: usize,
    delimiter: u8,
    state: State,
}

impl<'a> DelimiterIndices<'a> {
    fn new(input: &'a str, delimiter: u8) -> Self {
        debug_assert!(delimiter.is_ascii());
        Self {
            input,
            cursor: 0,
            delimiter,
            state: State::Outside,
        }
    }
}

impl Iterator for DelimiterIndices<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        let bytes = self.input.as_bytes();
        while let Some(&byte) = bytes.get(self.cursor) {
            let byte_index = self.cursor;
            self.cursor += 1;

            if self.state.is_outside() && byte == self.delimiter {
                return Some(byte_index);
            }
            self.state = self.state.feed(byte);
        }
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Segment<'a> {
    pub text: &'a str,
    pub start_byte: usize,
}

#[must_use]
pub(crate) fn first_unquoted_delimiter_index(input: &str, delimiter: u8) -> Option<usize> {
    DelimiterIndices::new(input, delimiter).next()
}

pub(crate) fn split_on_unquoted_delimiter(
    input: &str,
    delimiter: u8,
) -> impl Iterator<Item = Segment<'_>> {
    let mut indices = DelimiterIndices::new(input, delimiter);
    let mut start_byte = Some(0);

    std::iter::from_fn(move || {
        let segment_start = start_byte?;
        let segment_end = if let Some(delimiter_byte) = indices.next() {
            start_byte = Some(delimiter_byte + 1);
            delimiter_byte
        } else {
            start_byte = None;
            input.len()
        };
        Some(Segment {
            text: &input[segment_start..segment_end],
            start_byte: segment_start,
        })
    })
}

pub(crate) fn split_on_unquoted_whitespace(input: &str) -> impl Iterator<Item = Segment<'_>> {
    let bytes = input.as_bytes();
    let mut cursor = 0;

    std::iter::from_fn(move || {
        while cursor < bytes.len() {
            let width = whitespace_width_at(input, cursor);
            if width == 0 {
                break;
            }
            cursor += width;
        }
        if cursor >= bytes.len() {
            return None;
        }

        let start_byte = cursor;
        let mut state = State::Outside;
        while cursor < bytes.len() {
            if state.is_outside() && whitespace_width_at(input, cursor) > 0 {
                break;
            }
            state = state.feed_escaped(bytes[cursor]);
            cursor += 1;
        }

        Some(Segment {
            text: &input[start_byte..cursor],
            start_byte,
        })
    })
}

// Byte width of the Unicode whitespace character starting at `index`, or 0.
fn whitespace_width_at(input: &str, index: usize) -> usize {
    match input.as_bytes()[index] {
        b'\t'..=b'\r' | b' ' => 1,
        // Other ASCII bytes and UTF-8 continuation bytes never start whitespace.
        ..0xC0 => 0,
        _ => input[index..]
            .chars()
            .next()
            .filter(|ch| ch.is_whitespace())
            .map_or(0, char::len_utf8),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_string_recognizes_single_quoted_values() {
        let value = TemplateString::parse(
            "'images/logo.png'",
            Span::saturating_from_parts_usize(0, 17),
        );

        assert_eq!(
            value,
            TemplateString::Quoted {
                value: "images/logo.png",
                span: Span::saturating_from_parts_usize(1, 15),
            }
        );
    }

    #[test]
    fn template_string_recognizes_double_quoted_values() {
        let value =
            TemplateString::parse(r#""base.html""#, Span::saturating_from_parts_usize(0, 11));

        assert_eq!(
            value,
            TemplateString::Quoted {
                value: "base.html",
                span: Span::saturating_from_parts_usize(1, 9),
            }
        );
    }

    #[test]
    fn template_string_records_empty_span() {
        let value = TemplateString::parse(r#""""#, Span::saturating_from_parts_usize(10, 2));

        assert_eq!(
            value,
            TemplateString::Quoted {
                value: "",
                span: Span::saturating_from_parts_usize(11, 0),
            }
        );
    }

    #[test]
    fn template_string_accounts_for_trimmed_leading_bytes() {
        let value =
            TemplateString::parse("  'base.html' ", Span::saturating_from_parts_usize(40, 14));

        assert_eq!(
            value,
            TemplateString::Quoted {
                value: "base.html",
                span: Span::saturating_from_parts_usize(43, 9),
            }
        );
    }

    #[test]
    fn template_string_records_non_ascii_byte_length() {
        let value = TemplateString::parse(r#""é.html""#, Span::saturating_from_parts_usize(0, 9));

        assert_eq!(
            value,
            TemplateString::Quoted {
                value: "é.html",
                span: Span::saturating_from_parts_usize(1, 7),
            }
        );
    }

    #[test]
    fn template_string_leaves_unterminated_quotes_unquoted() {
        let value = TemplateString::parse("'base.html", Span::saturating_from_parts_usize(0, 10));

        assert_eq!(value, TemplateString::Unquoted("'base.html"));
    }

    #[test]
    fn template_string_leaves_bare_values_unquoted() {
        let value = TemplateString::parse("user.name", Span::saturating_from_parts_usize(0, 9));

        assert_eq!(value, TemplateString::Unquoted("user.name"));
    }

    fn split_on_unquoted_whitespace_text(input: &str) -> Vec<String> {
        split_on_unquoted_whitespace(input)
            .map(|segment| segment.text.to_owned())
            .collect()
    }

    #[test]
    fn first_unquoted_delimiter_index_returns_first_delimiter() {
        assert_eq!(first_unquoted_delimiter_index("a|b|c", b'|'), Some(1));
    }

    #[test]
    fn split_unquoted_delimiters() {
        let segments = split_on_unquoted_delimiter("a|b|c", b'|').collect::<Vec<_>>();
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
        let segments = split_on_unquoted_delimiter("a|'b|c'|d", b'|').collect::<Vec<_>>();
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
        let segments = split_on_unquoted_delimiter(r#"a|"b|c"|d"#, b'|').collect::<Vec<_>>();
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
        let segments = split_on_unquoted_delimiter(r#""a\"b"|c"#, b'|').collect::<Vec<_>>();

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
    fn split_unquoted_whitespace_simple() {
        assert_eq!(
            split_on_unquoted_whitespace_text("load i18n l10n"),
            vec!["load", "i18n", "l10n"]
        );
    }

    #[test]
    fn split_unquoted_whitespace_keeps_quoted_text_together() {
        assert_eq!(
            split_on_unquoted_whitespace_text(r#"if x == "hello world""#),
            vec!["if", "x", "==", r#""hello world""#]
        );
    }

    #[test]
    fn split_unquoted_whitespace_honors_escaped_quotes() {
        assert_eq!(
            split_on_unquoted_whitespace_text(r#"blocktrans "it\"s fine""#),
            vec!["blocktrans", r#""it\"s fine""#]
        );
    }

    #[test]
    fn split_unquoted_whitespace_handles_non_ascii_whitespace() {
        assert_eq!(
            split_on_unquoted_whitespace_text("a\u{a0}'b\u{3000}c'\u{3000}é"),
            vec!["a", "'b\u{3000}c'", "é"]
        );
    }

    #[test]
    fn split_unquoted_whitespace_empty() {
        assert!(split_on_unquoted_whitespace_text("").is_empty());
        assert!(split_on_unquoted_whitespace_text("   ").is_empty());
    }

    #[test]
    fn split_unquoted_whitespace_offsets() {
        let segments =
            split_on_unquoted_whitespace(r#"  url "view name" as view"#).collect::<Vec<_>>();

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

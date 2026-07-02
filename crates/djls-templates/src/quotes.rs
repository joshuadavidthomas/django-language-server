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

struct DelimiterIndices<'a> {
    input: &'a str,
    cursor: usize,
    delimiter: char,
    state: State,
}

impl<'a> DelimiterIndices<'a> {
    fn new(input: &'a str, delimiter: char) -> Self {
        Self {
            input,
            cursor: 0,
            delimiter,
            state: State::Outside,
        }
    }

    fn segments(mut self) -> Vec<Segment<'a>> {
        let mut segments = Vec::with_capacity((self.input.len() / 8).clamp(2, 8));
        let mut start_byte = 0;
        let delimiter_len = self.delimiter.len_utf8();

        while let Some(delimiter_byte) = self.next() {
            segments.push(Segment {
                text: &self.input[start_byte..delimiter_byte],
                start_byte,
            });
            start_byte = delimiter_byte + delimiter_len;
        }

        segments.push(Segment {
            text: &self.input[start_byte..],
            start_byte,
        });
        segments
    }
}

impl Iterator for DelimiterIndices<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        let cursor = self.cursor;
        for (relative_byte, ch) in self.input[cursor..].char_indices() {
            let byte_index = cursor + relative_byte;
            self.cursor = byte_index + ch.len_utf8();

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

#[must_use]
pub(crate) fn first_unquoted_delimiter_index(input: &str, delimiter: char) -> Option<usize> {
    DelimiterIndices::new(input, delimiter).next()
}

pub(crate) fn split_on_unquoted_delimiter(input: &str, delimiter: char) -> Vec<Segment<'_>> {
    DelimiterIndices::new(input, delimiter).segments()
}

pub(crate) fn split_on_unquoted_whitespace(input: &str) -> Vec<Segment<'_>> {
    let mut segments = Vec::with_capacity((input.len() / 8).clamp(2, 8));
    let mut start_byte = None;
    let mut state = State::Outside;

    for (byte_index, ch) in input.char_indices() {
        if state.is_outside() && ch.is_whitespace() {
            if let Some(segment_start_byte) = start_byte.take() {
                segments.push(Segment {
                    text: &input[segment_start_byte..byte_index],
                    start_byte: segment_start_byte,
                });
            }
            continue;
        }

        if start_byte.is_none() {
            start_byte = Some(byte_index);
        }
        state = state.feed_escaped(ch);
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
            .into_iter()
            .map(|segment| segment.text.to_owned())
            .collect()
    }

    #[test]
    fn first_unquoted_delimiter_index_returns_first_delimiter() {
        assert_eq!(first_unquoted_delimiter_index("a|b|c", '|'), Some(1));
    }

    #[test]
    fn split_unquoted_delimiters() {
        let segments = split_on_unquoted_delimiter("a|b|c", '|');
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
        let segments = split_on_unquoted_delimiter("a|'b|c'|d", '|');
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
        let segments = split_on_unquoted_delimiter(r#"a|"b|c"|d"#, '|');
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
        let segments = split_on_unquoted_delimiter(r#""a\"b"|c"#, '|');

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
    fn split_unquoted_whitespace_empty() {
        assert!(split_on_unquoted_whitespace_text("").is_empty());
        assert!(split_on_unquoted_whitespace_text("   ").is_empty());
    }

    #[test]
    fn split_unquoted_whitespace_offsets() {
        let segments = split_on_unquoted_whitespace(r#"  url "view name" as view"#);

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

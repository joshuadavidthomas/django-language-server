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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QuoteTracker {
    state: State,
}

impl QuoteTracker {
    pub(crate) fn new() -> Self {
        Self {
            state: State::Outside,
        }
    }

    pub(crate) fn is_outside(self) -> bool {
        self.state.is_outside()
    }

    fn feed(&mut self, ch: char) {
        self.state = self.state.feed(ch);
    }

    pub(crate) fn feed_escaped(&mut self, ch: char) {
        self.state = self.state.feed_escaped(ch);
    }
}

/// Byte indices of delimiter characters outside quoted regions.
pub(crate) struct DelimiterIndices<'a> {
    input: &'a str,
    cursor: usize,
    delimiter: char,
    quotes: QuoteTracker,
}

impl<'a> DelimiterIndices<'a> {
    pub(crate) fn new(input: &'a str, delimiter: char) -> Self {
        Self {
            input,
            cursor: 0,
            delimiter,
            quotes: QuoteTracker::new(),
        }
    }

    pub(crate) fn segments(mut self) -> Vec<Segment<'a>> {
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

            if self.quotes.is_outside() && ch == self.delimiter {
                return Some(byte_index);
            }
            self.quotes.feed(ch);
        }
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Segment<'a> {
    pub text: &'a str,
    pub start_byte: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn delimiter_indices_returns_first_delimiter() {
        assert_eq!(DelimiterIndices::new("a|b|c", '|').next(), Some(1));
    }

    #[test]
    fn split_unquoted_delimiters() {
        let segments = DelimiterIndices::new("a|b|c", '|').segments();
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
        let segments = DelimiterIndices::new("a|'b|c'|d", '|').segments();
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
        let segments = DelimiterIndices::new(r#"a|"b|c"|d"#, '|').segments();
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
        let segments = DelimiterIndices::new(r#""a\"b"|c"#, '|').segments();

        // With literal backslashes, \" closes the quote, then b" opens a new one.
        assert_eq!(
            segments,
            vec![Segment {
                text: r#""a\"b"|c"#,
                start_byte: 0,
            }]
        );
    }
}

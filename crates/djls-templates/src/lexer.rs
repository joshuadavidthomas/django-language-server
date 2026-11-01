use djls_source::Span;
use memchr::memchr;
use memchr::memchr3;

use crate::tokens::TagDelimiter;
use crate::tokens::Token;
use crate::tokens::TokenStream;

/// Whether a Django tag, variable, or comment may span lines.
///
/// Django splits templates with `django.template.base.tag_re`,
/// `{%.*?%}|{{.*?}}|{#.*?#}`. Its `.` stops at `\n`, so by default a construct
/// must close on the line where it opens. Projects that compile `tag_re` with
/// `re.DOTALL` let constructs span lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LexerMode {
    /// Django's default `tag_re`: a closer must appear before the next `\n`.
    SingleLine,
    /// `tag_re` compiled with `re.DOTALL`: a closer may appear on any later line.
    MultiLine,
}

// Closing delimiter kinds proven absent between the cursor and a search boundary.
//
// The cursor only advances through immutable source, so a closer absent before
// a boundary stays absent for every later search that ends at the same boundary.
#[derive(Default)]
struct MissingTagClosers([Option<usize>; 3]);

impl MissingTagClosers {
    fn index(delimiter: TagDelimiter) -> usize {
        match delimiter {
            TagDelimiter::Block => 0,
            TagDelimiter::Variable => 1,
            TagDelimiter::Comment => 2,
        }
    }

    fn contains(&self, delimiter: TagDelimiter, boundary: usize) -> bool {
        self.0[Self::index(delimiter)] == Some(boundary)
    }

    fn insert(&mut self, delimiter: TagDelimiter, boundary: usize) {
        self.0[Self::index(delimiter)] = Some(boundary);
    }
}

pub(crate) struct Lexer<'src> {
    source: &'src str,
    mode: LexerMode,
    start: usize,
    current: usize,
    // End of the line containing `current`, cached so a line with many unmatched
    // openers is scanned for its newline once.
    line_end: usize,
}

impl<'src> Lexer<'src> {
    #[must_use]
    pub(crate) fn new(source: &'src str, mode: LexerMode) -> Self {
        Lexer {
            source,
            mode,
            start: 0,
            current: 0,
            line_end: 0,
        }
    }

    pub(crate) fn tokenize(&mut self) -> Vec<Token<&'src str>> {
        let mut tokens = TokenStream::with_estimated_capacity(self.source);
        let mut missing_closers = MissingTagClosers::default();

        while !self.is_at_end() {
            self.start = self.current;

            let token = match self.peek() {
                TagDelimiter::CHAR_OPEN => {
                    let remaining = self.remaining_source();

                    match TagDelimiter::from_input(remaining) {
                        Some(delimiter) => self.lex_django_tag(delimiter, &mut missing_closers),
                        None => self.lex_text(),
                    }
                }
                c if c.is_whitespace() => self.lex_whitespace(c),
                _ => self.lex_text(),
            };

            tokens.push(token);
        }

        tokens.push(Token::Eof);

        tokens.into()
    }

    fn lex_django_tag(
        &mut self,
        delimiter: TagDelimiter,
        missing_closers: &mut MissingTagClosers,
    ) -> Token<&'src str> {
        let content_start = self.start + TagDelimiter::LENGTH;
        self.consume_n(TagDelimiter::LENGTH);

        if let Some(content_end) = self.find_closer(delimiter, content_start, missing_closers) {
            self.current = content_end + delimiter.closer().len();
            let content = &self.source[content_start..content_end];
            let span = Span::saturating_from_bounds_usize(content_start, content_end);
            return match delimiter {
                TagDelimiter::Block => Token::Block { content, span },
                TagDelimiter::Variable => Token::Variable { content, span },
                TagDelimiter::Comment => Token::Comment { content, span },
            };
        }

        // Django's `tag_re` retries one character later when an opener has
        // no closer. If an opener starts there (`{{%`, `{{#`, `{{{`), lex it
        // as the next token and leave this `{` as text, as Django does.
        // Otherwise keep the Error token as an editor hint for the
        // unclosed opener, even though Django renders it as text.
        let next = self.start + TagDelimiter::CHAR_OPEN.len_utf8();
        if TagDelimiter::from_input(&self.source[next..]).is_some() {
            self.current = next;
            return Token::Text {
                content: &self.source[self.start..next],
                span: Span::saturating_from_bounds_usize(self.start, next),
            };
        }

        self.current += self.consume_until_stop_char();
        let content = self.consumed_source_from(content_start);
        let span = Span::saturating_from_bounds_usize(content_start, self.current);
        Token::Error {
            content,
            span,
            delimiter,
        }
    }

    fn lex_whitespace(&mut self, c: char) -> Token<&'src str> {
        self.consume();

        if c == '\n' || c == '\r' {
            if c == '\r' && self.peek() == '\n' {
                self.consume();
            }
            let span = Span::saturating_from_bounds_usize(self.start, self.current);
            return Token::Newline { span };
        }

        while !self.is_at_end() {
            let remaining = self.remaining_source().as_bytes();

            match remaining.first() {
                Some(&b'\n' | &b'\r') | None => break,
                Some(&b' ' | &b'\t') => self.current += 1,
                Some(_) => {
                    if !self.peek().is_whitespace() {
                        break;
                    }
                    self.consume();
                }
            }
        }

        let span = Span::saturating_from_bounds_usize(self.start, self.current);
        Token::Whitespace { span }
    }

    fn lex_text(&mut self) -> Token<&'src str> {
        let text_start = self.current;
        self.current += self.consume_until_stop_char();
        let text = self.consumed_source_from(text_start);
        let span = Span::saturating_from_bounds_usize(self.start, self.current);
        Token::Text {
            content: text,
            span,
        }
    }

    #[inline]
    fn peek(&self) -> char {
        self.remaining_source().chars().next().unwrap_or('\0')
    }

    #[inline]
    fn remaining_source(&self) -> &'src str {
        &self.source[self.current..]
    }

    #[inline]
    fn consumed_source_from(&self, start: usize) -> &'src str {
        &self.source[start..self.current]
    }

    #[inline]
    fn is_at_end(&self) -> bool {
        self.current >= self.source.len()
    }

    #[inline]
    fn consume(&mut self) {
        if let Some(ch) = self.remaining_source().chars().next() {
            self.current += ch.len_utf8();
        }
    }

    fn consume_n(&mut self, count: usize) {
        for _ in 0..count {
            self.consume();
        }
    }

    /// Return where the content opened at `from` ends, if its closer appears
    /// before the search boundary.
    fn find_closer(
        &mut self,
        delimiter: TagDelimiter,
        from: usize,
        missing_closers: &mut MissingTagClosers,
    ) -> Option<usize> {
        let boundary = self.closer_search_boundary();
        if missing_closers.contains(delimiter, boundary) {
            return None;
        }
        let found = delimiter
            .find_closer(&self.source[from..boundary])
            .map(|pos| from + pos);
        if found.is_none() {
            missing_closers.insert(delimiter, boundary);
        }
        found
    }

    fn closer_search_boundary(&mut self) -> usize {
        match self.mode {
            LexerMode::MultiLine => self.source.len(),
            LexerMode::SingleLine => {
                if self.current >= self.line_end {
                    self.line_end = memchr(b'\n', self.remaining_source().as_bytes())
                        .map_or(self.source.len(), |pos| self.current + pos);
                }
                self.line_end
            }
        }
    }

    fn consume_until_stop_char(&self) -> usize {
        let mut offset = 0;
        let max = self.source.len() - self.current;

        while offset < max {
            let remaining = &self.remaining_source()[offset..];

            match memchr3(b'{', b'\n', b'\r', remaining.as_bytes()) {
                None => {
                    offset = max;
                    break;
                }
                Some(pos) => {
                    let is_newline = matches!(remaining.as_bytes()[pos], b'\n' | b'\r');
                    let is_django_delimiter = TagDelimiter::from_input(&remaining[pos..]).is_some();

                    if is_newline || is_django_delimiter {
                        offset += pos;
                        break;
                    }

                    offset += pos + 1;
                }
            }
        }

        offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_tag_closers_are_independent_per_delimiter_and_boundary() {
        let mut missing_closers = MissingTagClosers::default();
        missing_closers.insert(TagDelimiter::Variable, 10);
        assert!(missing_closers.contains(TagDelimiter::Variable, 10));
        assert!(!missing_closers.contains(TagDelimiter::Variable, 20));
        assert!(!missing_closers.contains(TagDelimiter::Block, 10));
        assert!(!missing_closers.contains(TagDelimiter::Comment, 10));

        missing_closers.insert(TagDelimiter::Block, 10);
        missing_closers.insert(TagDelimiter::Variable, 20);
        assert!(!missing_closers.contains(TagDelimiter::Variable, 10));
        assert!(missing_closers.contains(TagDelimiter::Variable, 20));
        assert!(missing_closers.contains(TagDelimiter::Block, 10));
        assert!(!missing_closers.contains(TagDelimiter::Comment, 10));
    }

    #[test]
    fn public_tokens_own_their_contents() {
        let tokens = {
            let source = String::from("a\n{{v}}{%t%}{#c#}{{x");
            crate::lex_template_impl(&source, LexerMode::MultiLine)
        };
        assert_eq!(
            tokens,
            vec![
                Token::Text {
                    content: "a".into(),
                    span: Span::new(0, 1)
                },
                Token::Newline {
                    span: Span::new(1, 1)
                },
                Token::Variable {
                    content: "v".into(),
                    span: Span::new(4, 1)
                },
                Token::Block {
                    content: "t".into(),
                    span: Span::new(9, 1)
                },
                Token::Comment {
                    content: "c".into(),
                    span: Span::new(14, 1)
                },
                Token::Error {
                    content: "x".into(),
                    span: Span::new(19, 1),
                    delimiter: TagDelimiter::Variable
                },
                Token::Eof,
            ]
        );
    }

    #[test]
    fn missing_closers_are_independent_and_preserve_recovery_spans() {
        let mut lexer = Lexer::new("{{é\r\n{%x%}{#y#}{{z", LexerMode::MultiLine);
        assert_eq!(
            lexer.tokenize(),
            vec![
                Token::Error {
                    content: "é",
                    span: Span::new(2, 2),
                    delimiter: TagDelimiter::Variable,
                },
                Token::Newline {
                    span: Span::new(4, 2)
                },
                Token::Block {
                    content: "x",
                    span: Span::new(8, 1)
                },
                Token::Comment {
                    content: "y",
                    span: Span::new(13, 1)
                },
                Token::Error {
                    content: "z",
                    span: Span::new(18, 1),
                    delimiter: TagDelimiter::Variable,
                },
                Token::Eof,
            ]
        );
    }

    #[test]
    fn late_closers_still_cross_newlines_and_nested_openers() {
        for (source, expected) in [
            (
                "{{é\r\n{{z}}",
                Token::Variable {
                    content: "é\r\n{{z",
                    span: Span::new(2, 7),
                },
            ),
            (
                "{%é\r\n{%z%}",
                Token::Block {
                    content: "é\r\n{%z",
                    span: Span::new(2, 7),
                },
            ),
            (
                "{#é\r\n{#z#}",
                Token::Comment {
                    content: "é\r\n{#z",
                    span: Span::new(2, 7),
                },
            ),
        ] {
            assert_eq!(
                Lexer::new(source, LexerMode::MultiLine).tokenize(),
                vec![expected, Token::Eof]
            );
        }
    }

    // Django's `django.template.base.tag_re`, with and without `re.DOTALL`.
    // Rust's `regex` uses leftmost-first alternation like Python's `re`.
    fn django_tag_re(mode: LexerMode) -> regex::Regex {
        let flags = match mode {
            LexerMode::SingleLine => "",
            LexerMode::MultiLine => "(?s)",
        };
        regex::Regex::new(&format!(r"{flags}\{{%.*?%\}}|\{{\{{.*?\}}\}}|\{{#.*?#\}}"))
            .expect("tag_re should compile")
    }

    fn lexed_tag_spans(source: &str, mode: LexerMode) -> Vec<(usize, usize)> {
        Lexer::new(source, mode)
            .tokenize()
            .iter()
            .filter(|token| {
                matches!(
                    token,
                    Token::Block { .. } | Token::Variable { .. } | Token::Comment { .. }
                )
            })
            .map(|token| {
                let span = token.full_span_or_fallback();
                (span.start() as usize, span.end() as usize)
            })
            .collect()
    }

    #[test]
    fn tags_match_django_tag_re_in_both_modes() {
        const ALPHABET: [char; 7] = ['{', '%', '#', '}', '\n', '\r', 'x'];
        const MAX_LEN: u32 = 6;

        let tag_res =
            [LexerMode::SingleLine, LexerMode::MultiLine].map(|mode| (mode, django_tag_re(mode)));
        let mut mismatches = Vec::new();
        for len in 1..=MAX_LEN {
            for index in 0..ALPHABET.len().pow(len) {
                let mut rest = index;
                let source: String = (0..len)
                    .map(|_| {
                        let ch = ALPHABET[rest % ALPHABET.len()];
                        rest /= ALPHABET.len();
                        ch
                    })
                    .collect();
                for (mode, tag_re) in &tag_res {
                    let mode = *mode;
                    let expected: Vec<_> = tag_re
                        .find_iter(&source)
                        .map(|found| (found.start(), found.end()))
                        .collect();
                    if lexed_tag_spans(&source, mode) != expected && mismatches.len() < 10 {
                        mismatches.push((source.clone(), mode, expected));
                    }
                }
            }
        }
        assert!(mismatches.is_empty(), "{mismatches:#?}");
    }

    #[test]
    fn single_line_tags_close_before_the_next_newline() {
        // Default Django sees text, then `{% if x %}`; DOTALL sees one tag named `}}`.
        let source = "{%}}\n{% if x %}";
        assert_eq!(
            Lexer::new(source, LexerMode::SingleLine).tokenize(),
            vec![
                Token::Error {
                    content: "}}",
                    span: Span::new(2, 2),
                    delimiter: TagDelimiter::Block,
                },
                Token::Newline {
                    span: Span::new(4, 1)
                },
                Token::Block {
                    content: " if x ",
                    span: Span::new(7, 6),
                },
                Token::Eof,
            ]
        );
        assert_eq!(
            Lexer::new(source, LexerMode::MultiLine).tokenize(),
            vec![
                Token::Block {
                    content: "}}\n{% if x ",
                    span: Span::new(2, 11),
                },
                Token::Eof,
            ]
        );
    }

    #[test]
    fn unclosed_opener_yields_brace_to_overlapping_opener_like_django() {
        let text = |start| Token::Text {
            content: "{",
            span: Span::new(start, 1),
        };
        for (source, expected) in [
            (
                "{{% block a %}",
                vec![
                    text(0),
                    Token::Block {
                        content: " block a ",
                        span: Span::new(3, 9),
                    },
                ],
            ),
            (
                "{{#x#}",
                vec![
                    text(0),
                    Token::Comment {
                        content: "x",
                        span: Span::new(3, 1),
                    },
                ],
            ),
            (
                "{{{% if %}",
                vec![
                    text(0),
                    text(1),
                    Token::Block {
                        content: " if ",
                        span: Span::new(4, 4),
                    },
                ],
            ),
            (
                "{{% if",
                vec![
                    text(0),
                    Token::Error {
                        content: " if",
                        span: Span::new(3, 3),
                        delimiter: TagDelimiter::Block,
                    },
                ],
            ),
            (
                "{{% if %}}",
                vec![Token::Variable {
                    content: "% if %",
                    span: Span::new(2, 6),
                }],
            ),
            (
                "{{ x {% if %}",
                vec![
                    Token::Error {
                        content: " x ",
                        span: Span::new(2, 3),
                        delimiter: TagDelimiter::Variable,
                    },
                    Token::Block {
                        content: " if ",
                        span: Span::new(7, 4),
                    },
                ],
            ),
        ] {
            let mut expected = expected;
            expected.push(Token::Eof);
            assert_eq!(
                Lexer::new(source, LexerMode::MultiLine).tokenize(),
                expected,
                "{source:?}"
            );
        }
    }

    #[test]
    fn repeated_unmatched_delimiters_recover_at_each_opener() {
        for repetitions in [4096, 8192] {
            let source = "{{x\r\n{%é\n{#z\n".repeat(repetitions);
            let mut lexer = Lexer::new(&source, LexerMode::MultiLine);
            let tokens = lexer.tokenize();
            assert_eq!(tokens.len(), 6 * repetitions + 1);
            assert_eq!(
                tokens
                    .iter()
                    .filter(|token| matches!(token, Token::Error { .. }))
                    .count(),
                3 * repetitions
            );
        }
    }

    #[derive(serde::Serialize)]
    struct ContentToken<'a> {
        content: &'a str,
        span: (u32, u32),
        full_span: (u32, u32),
    }

    #[derive(serde::Serialize)]
    struct SpanToken {
        span: (u32, u32),
    }

    impl<T: AsRef<str>> serde::Serialize for Token<T> {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            match self {
                Token::Block { content, span } => serializer.serialize_newtype_variant(
                    "Token",
                    0,
                    "Block",
                    &ContentToken {
                        content: content.as_ref(),
                        span: span.into(),
                        full_span: self.full_span_or_fallback().into(),
                    },
                ),
                Token::Comment { content, span } => serializer.serialize_newtype_variant(
                    "Token",
                    1,
                    "Comment",
                    &ContentToken {
                        content: content.as_ref(),
                        span: span.into(),
                        full_span: self.full_span_or_fallback().into(),
                    },
                ),
                Token::Eof => serializer.serialize_unit_variant("Token", 2, "Eof"),
                Token::Error { content, span, .. } => serializer.serialize_newtype_variant(
                    "Token",
                    3,
                    "Error",
                    &ContentToken {
                        content: content.as_ref(),
                        span: span.into(),
                        full_span: self.full_span_or_fallback().into(),
                    },
                ),
                Token::Newline { span } => serializer.serialize_newtype_variant(
                    "Token",
                    4,
                    "Newline",
                    &SpanToken { span: span.into() },
                ),
                Token::Text { content, span } => serializer.serialize_newtype_variant(
                    "Token",
                    5,
                    "Text",
                    &ContentToken {
                        content: content.as_ref(),
                        span: span.into(),
                        full_span: span.into(),
                    },
                ),
                Token::Variable { content, span } => serializer.serialize_newtype_variant(
                    "Token",
                    6,
                    "Variable",
                    &ContentToken {
                        content: content.as_ref(),
                        span: span.into(),
                        full_span: self.full_span_or_fallback().into(),
                    },
                ),
                Token::Whitespace { span } => serializer.serialize_newtype_variant(
                    "Token",
                    7,
                    "Whitespace",
                    &SpanToken { span: span.into() },
                ),
            }
        }
    }

    #[test]
    fn test_tokenize_html() {
        let source = r#"<div class="container" id="main" disabled></div>"#;
        let mut lexer = Lexer::new(source, LexerMode::MultiLine);
        let snapshot = lexer.tokenize();
        insta::assert_yaml_snapshot!(snapshot);
    }

    #[test]
    fn test_tokenize_django_variable() {
        let source = "{{ user.name|default:\"Anonymous\"|title }}";
        let mut lexer = Lexer::new(source, LexerMode::MultiLine);
        let snapshot = lexer.tokenize();
        insta::assert_yaml_snapshot!(snapshot);
    }

    #[test]
    fn test_tokenize_django_block() {
        let source = "{% if user.is_staff %}Admin{% else %}User{% endif %}";
        let mut lexer = Lexer::new(source, LexerMode::MultiLine);
        let snapshot = lexer.tokenize();
        insta::assert_yaml_snapshot!(snapshot);
    }

    #[test]
    fn test_tokenize_comments() {
        let source = r"<!-- HTML comment -->
{# Django comment #}
<script>
    // JS single line comment
    /* JS multi-line
       comment */
</script>
<style>
    /* CSS comment */
</style>";
        let mut lexer = Lexer::new(source, LexerMode::MultiLine);
        let snapshot = lexer.tokenize();
        insta::assert_yaml_snapshot!(snapshot);
    }

    #[test]
    fn test_tokenize_script() {
        let source = r#"<script type="text/javascript">
    // Single line comment
    const x = 1;
    /* Multi-line
       comment */
    console.log(x);
</script>"#;
        let mut lexer = Lexer::new(source, LexerMode::MultiLine);
        let snapshot = lexer.tokenize();
        insta::assert_yaml_snapshot!(snapshot);
    }

    #[test]
    fn test_tokenize_style() {
        let source = r#"<style type="text/css">
    /* Header styles */
    .header {
        color: blue;
    }
</style>"#;
        let mut lexer = Lexer::new(source, LexerMode::MultiLine);
        let snapshot = lexer.tokenize();
        insta::assert_yaml_snapshot!(snapshot);
    }

    #[test]
    fn test_tokenize_nested_delimiters() {
        let source = r"{{ user.name }}
{% if true %}
{# comment #}
<!-- html comment -->
<div>text</div>";
        let mut lexer = Lexer::new(source, LexerMode::MultiLine);
        let snapshot = lexer.tokenize();
        insta::assert_yaml_snapshot!(snapshot);
    }

    #[test]
    fn test_tokenize_everything() {
        let source = r#"<!DOCTYPE html>
<html>
<head>
    <style type="text/css">
        /* Style header */
        .header { color: blue; }
    </style>
    <script type="text/javascript">
        // Init app
        const app = {
            /* Config */
            debug: true
        };
    </script>
</head>
<body>
    <!-- Header section -->
    <div class="header" id="main" data-value="123" disabled>
        {% if user.is_authenticated %}
            {# Welcome message #}
            <h1>Welcome, {{ user.name|default:"Guest"|title }}!</h1>
            {% if user.is_staff %}
                <span>Admin</span>
            {% else %}
                <span>User</span>
            {% endif %}
        {% endif %}
    </div>
</body>
</html>"#;
        let mut lexer = Lexer::new(source, LexerMode::MultiLine);
        let snapshot = lexer.tokenize();
        insta::assert_yaml_snapshot!(snapshot);
    }

    #[test]
    fn test_tokenize_unclosed_style() {
        let source = "<style>body { color: blue; ";
        let mut lexer = Lexer::new(source, LexerMode::MultiLine);
        let snapshot = lexer.tokenize();
        insta::assert_yaml_snapshot!(snapshot);
    }
}

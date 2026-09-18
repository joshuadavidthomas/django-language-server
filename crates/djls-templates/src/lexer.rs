use djls_source::Span;
use memchr::memchr3;

use crate::tokens::TagDelimiter;
use crate::tokens::Token;
use crate::tokens::TokenStream;

// A compact set of closing delimiter kinds proven absent from the remaining source.
#[derive(Default)]
struct MissingTagClosers(u8);

impl MissingTagClosers {
    fn mask(delimiter: TagDelimiter) -> u8 {
        match delimiter {
            TagDelimiter::Block => 0b001,
            TagDelimiter::Variable => 0b010,
            TagDelimiter::Comment => 0b100,
        }
    }

    fn contains(&self, delimiter: TagDelimiter) -> bool {
        self.0 & Self::mask(delimiter) != 0
    }

    fn insert(&mut self, delimiter: TagDelimiter) {
        self.0 |= Self::mask(delimiter);
    }
}

pub(crate) struct Lexer<'src> {
    source: &'src str,
    start: usize,
    current: usize,
}

impl<'src> Lexer<'src> {
    #[must_use]
    pub(crate) fn new(source: &'src str) -> Self {
        Lexer {
            source,
            start: 0,
            current: 0,
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

        match self.consume_until_delimiter(delimiter, missing_closers) {
            Ok(content) => {
                let len = content.len();
                let span = Span::saturating_from_parts_usize(content_start, len);
                self.consume_n(delimiter.closer().len());
                match delimiter {
                    TagDelimiter::Block => Token::Block { content, span },
                    TagDelimiter::Variable => Token::Variable { content, span },
                    TagDelimiter::Comment => Token::Comment { content, span },
                }
            }
            Err(err_text) => {
                let len = err_text.len();
                let span = if len == 0 {
                    Span::saturating_from_bounds_usize(content_start, self.current)
                } else {
                    Span::saturating_from_parts_usize(content_start, len)
                };
                Token::Error {
                    content: err_text,
                    span,
                    delimiter,
                }
            }
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

    fn consume_until_delimiter(
        &mut self,
        delimiter: TagDelimiter,
        missing_closers: &mut MissingTagClosers,
    ) -> Result<&'src str, &'src str> {
        let offset = self.current;

        if !missing_closers.contains(delimiter) {
            if let Some(pos) = delimiter.find_closer(self.remaining_source()) {
                self.current += pos;
                return Ok(self.consumed_source_from(offset));
            }
            // The cursor only advances through immutable source, so a closer
            // absent from this suffix is absent from every later suffix too.
            missing_closers.insert(delimiter);
        }

        self.current += self.consume_until_stop_char();
        Err(self.consumed_source_from(offset))
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
    fn missing_tag_closers_accumulate_independently() {
        let mut missing_closers = MissingTagClosers::default();
        assert!(!missing_closers.contains(TagDelimiter::Variable));
        assert!(!missing_closers.contains(TagDelimiter::Block));
        assert!(!missing_closers.contains(TagDelimiter::Comment));

        missing_closers.insert(TagDelimiter::Variable);
        assert!(missing_closers.contains(TagDelimiter::Variable));
        assert!(!missing_closers.contains(TagDelimiter::Block));
        assert!(!missing_closers.contains(TagDelimiter::Comment));

        missing_closers.insert(TagDelimiter::Block);
        missing_closers.insert(TagDelimiter::Variable);
        assert!(missing_closers.contains(TagDelimiter::Variable));
        assert!(missing_closers.contains(TagDelimiter::Block));
        assert!(!missing_closers.contains(TagDelimiter::Comment));

        missing_closers.insert(TagDelimiter::Comment);
        assert!(missing_closers.contains(TagDelimiter::Variable));
        assert!(missing_closers.contains(TagDelimiter::Block));
        assert!(missing_closers.contains(TagDelimiter::Comment));
    }

    #[test]
    fn public_tokens_own_their_contents() {
        let tokens = {
            let source = String::from("a\n{{v}}{%t%}{#c#}{{x");
            crate::lex_template_impl(&source)
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
        let mut lexer = Lexer::new("{{é\r\n{%x%}{#y#}{{z");
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
            assert_eq!(Lexer::new(source).tokenize(), vec![expected, Token::Eof]);
        }
    }

    #[test]
    fn repeated_unmatched_delimiters_recover_at_each_opener() {
        for repetitions in [4096, 8192] {
            let source = "{{x\r\n{%é\n{#z\n".repeat(repetitions);
            let mut lexer = Lexer::new(&source);
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
        let mut lexer = Lexer::new(source);
        let snapshot = lexer.tokenize();
        insta::assert_yaml_snapshot!(snapshot);
    }

    #[test]
    fn test_tokenize_django_variable() {
        let source = "{{ user.name|default:\"Anonymous\"|title }}";
        let mut lexer = Lexer::new(source);
        let snapshot = lexer.tokenize();
        insta::assert_yaml_snapshot!(snapshot);
    }

    #[test]
    fn test_tokenize_django_block() {
        let source = "{% if user.is_staff %}Admin{% else %}User{% endif %}";
        let mut lexer = Lexer::new(source);
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
        let mut lexer = Lexer::new(source);
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
        let mut lexer = Lexer::new(source);
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
        let mut lexer = Lexer::new(source);
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
        let mut lexer = Lexer::new(source);
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
        let mut lexer = Lexer::new(source);
        let snapshot = lexer.tokenize();
        insta::assert_yaml_snapshot!(snapshot);
    }

    #[test]
    fn test_tokenize_unclosed_style() {
        let source = "<style>body { color: blue; ";
        let mut lexer = Lexer::new(source);
        let snapshot = lexer.tokenize();
        insta::assert_yaml_snapshot!(snapshot);
    }
}

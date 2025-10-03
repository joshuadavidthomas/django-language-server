use djls_source::Span;
use memchr::memchr3;
use memchr::memmem;

use crate::tokens::TagDelimiter;
use crate::tokens::Token;
use crate::tokens::TokenStream;

pub struct Lexer {
    source: String,
    start: usize,
    current: usize,
}

impl Lexer {
    #[must_use]
    pub fn new(source: &str) -> Self {
        Lexer {
            source: String::from(source),
            start: 0,
            current: 0,
        }
    }

    pub fn tokenize(&mut self) -> Vec<Token> {
        let mut tokens = TokenStream::with_estimated_capacity(&self.source);

        while !self.is_at_end() {
            self.start = self.current;

            let token =
                match self.peek() {
                    TagDelimiter::CHAR_OPEN => {
                        let remaining = self.remaining_source();

                        match TagDelimiter::from_input(remaining) {
                            Some(TagDelimiter::Block) => self
                                .lex_django_tag(TagDelimiter::Block, |content, span| {
                                    Token::Block { content, span }
                                }),
                            Some(TagDelimiter::Variable) => self
                                .lex_django_tag(TagDelimiter::Variable, |content, span| {
                                    Token::Variable { content, span }
                                }),
                            Some(TagDelimiter::Comment) => self
                                .lex_django_tag(TagDelimiter::Comment, |content, span| {
                                    Token::Comment { content, span }
                                }),
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
        token_fn: impl FnOnce(String, Span) -> Token,
    ) -> Token {
        let content_start = self.start + TagDelimiter::LENGTH;

        self.consume_n(TagDelimiter::LENGTH);

        match self.consume_until_delimiter(delimiter.closer()) {
            Ok(text) => {
                let len = text.len();
                let span = Span::saturating_from_parts_usize(content_start, len);
                self.consume_n(delimiter.closer().len());
                token_fn(text, span)
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
                }
            }
        }
    }

    fn lex_whitespace(&mut self, c: char) -> Token {
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

    fn lex_text(&mut self) -> Token {
        let text_start = self.current;
        self.current += self.consume_until_stop_char();
        let text = self.consumed_source_from(text_start);
        let span = Span::saturating_from_bounds_usize(self.start, self.current);
        Token::Text {
            content: text.to_string(),
            span,
        }
    }

    #[inline]
    fn peek(&self) -> char {
        self.remaining_source().chars().next().unwrap_or('\0')
    }

    #[inline]
    fn remaining_source(&self) -> &str {
        &self.source[self.current..]
    }

    #[inline]
    fn consumed_source_from(&self, start: usize) -> &str {
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

    fn consume_until_delimiter(&mut self, delimiter: &str) -> Result<String, String> {
        let offset = self.current;

        if let Some(pos) = memmem::find(self.remaining_source().as_bytes(), delimiter.as_bytes()) {
            self.current += pos;
            return Ok(self.consumed_source_from(offset).to_string());
        }

        self.current += self.consume_until_stop_char();
        Err(self.consumed_source_from(offset).to_string())
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
    use crate::tokens::TokenSnapshotVec;

    #[test]
    fn test_tokenize_html() {
        let source = r#"<div class="container" id="main" disabled></div>"#;
        let mut lexer = Lexer::new(source);
        let tokens = lexer.tokenize();
        let snapshot = TokenSnapshotVec(tokens).to_snapshot();
        insta::assert_yaml_snapshot!(snapshot);
    }

    #[test]
    fn test_tokenize_django_variable() {
        let source = "{{ user.name|default:\"Anonymous\"|title }}";
        let mut lexer = Lexer::new(source);
        let tokens = lexer.tokenize();
        let snapshot = TokenSnapshotVec(tokens).to_snapshot();
        insta::assert_yaml_snapshot!(snapshot);
    }

    #[test]
    fn test_tokenize_django_block() {
        let source = "{% if user.is_staff %}Admin{% else %}User{% endif %}";
        let mut lexer = Lexer::new(source);
        let tokens = lexer.tokenize();
        let snapshot = TokenSnapshotVec(tokens).to_snapshot();
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
        let tokens = lexer.tokenize();
        let snapshot = TokenSnapshotVec(tokens).to_snapshot();
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
        let tokens = lexer.tokenize();
        let snapshot = TokenSnapshotVec(tokens).to_snapshot();
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
        let tokens = lexer.tokenize();
        let snapshot = TokenSnapshotVec(tokens).to_snapshot();
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
        let tokens = lexer.tokenize();
        let snapshot = TokenSnapshotVec(tokens).to_snapshot();
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
        let tokens = lexer.tokenize();
        let snapshot = TokenSnapshotVec(tokens).to_snapshot();
        insta::assert_yaml_snapshot!(snapshot);
    }

    #[test]
    fn test_tokenize_unclosed_style() {
        let source = "<style>body { color: blue; ";
        let mut lexer = Lexer::new(source);
        let tokens = lexer.tokenize();
        let snapshot = TokenSnapshotVec(tokens).to_snapshot();
        insta::assert_yaml_snapshot!(snapshot);
    }
}

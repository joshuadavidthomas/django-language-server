use thiserror::Error;

use crate::tokens::Token;
use crate::tokens::TokenType;

pub struct Lexer {
    source: String,
    chars: Vec<char>,
    start: usize,
    current: usize,
    line: usize,
}

impl Lexer {
    #[must_use]
    pub fn new(source: &str) -> Self {
        Lexer {
            source: String::from(source),
            chars: source.chars().collect(),
            start: 0,
            current: 0,
            line: 1,
        }
    }

    #[allow(clippy::too_many_lines)]
    pub fn tokenize(&mut self) -> Result<Vec<Token>, LexerError> {
        let mut tokens = Vec::new();

        while !self.is_at_end() {
            self.start = self.current;

            let token_type = match self.peek()? {
                '{' => match self.peek_next()? {
                    '%' => {
                        self.consume_n(2)?; // {%
                        let content = self.consume_until("%}")?;
                        self.consume_n(2)?; // %}
                        TokenType::DjangoBlock(content)
                    }
                    '{' => {
                        self.consume_n(2)?; // {{
                        let content = self.consume_until("}}")?;
                        self.consume_n(2)?; // }}
                        TokenType::DjangoVariable(content)
                    }
                    '#' => {
                        self.consume_n(2)?; // {#
                        let content = self.consume_until("#}")?;
                        self.consume_n(2)?; // #}
                        TokenType::Comment(content, "{#".to_string(), Some("#}".to_string()))
                    }
                    _ => {
                        self.consume()?; // {
                        TokenType::Text(String::from("{"))
                    }
                },

                '<' => match self.peek_next()? {
                    '/' => {
                        self.consume_n(2)?; // </
                        let tag = self.consume_until(">")?;
                        self.consume()?; // >
                        TokenType::HtmlTagClose(tag)
                    }
                    '!' if self.matches("<!--") => {
                        self.consume_n(4)?; // <!--
                        let content = self.consume_until("-->")?;
                        self.consume_n(3)?; // -->
                        TokenType::Comment(content, "<!--".to_string(), Some("-->".to_string()))
                    }
                    _ => {
                        self.consume()?; // consume <
                        let tag = self.consume_until(">")?;
                        self.consume()?; // consume >
                        if tag.starts_with("script") {
                            TokenType::ScriptTagOpen(tag)
                        } else if tag.starts_with("style") {
                            TokenType::StyleTagOpen(tag)
                        } else if tag.ends_with('/') {
                            TokenType::HtmlTagVoid(tag.trim_end_matches('/').to_string())
                        } else {
                            TokenType::HtmlTagOpen(tag)
                        }
                    }
                },

                '/' => match self.peek_next()? {
                    '/' => {
                        self.consume_n(2)?; // //
                        let content = self.consume_until("\n")?;
                        TokenType::Comment(content, "//".to_string(), None)
                    }
                    '*' => {
                        self.consume_n(2)?; // /*
                        let content = self.consume_until("*/")?;
                        self.consume_n(2)?; // */
                        TokenType::Comment(content, "/*".to_string(), Some("*/".to_string()))
                    }
                    _ => {
                        self.consume()?;
                        TokenType::Text("/".to_string())
                    }
                },

                c if c.is_whitespace() => {
                    if c == '\n' || c == '\r' {
                        self.consume()?; // \r or \n
                        if c == '\r' && self.peek()? == '\n' {
                            self.consume()?; // \n of \r\n
                        }
                        TokenType::Newline
                    } else {
                        self.consume()?; // Consume the first whitespace
                        while !self.is_at_end() && self.peek()?.is_whitespace() {
                            if self.peek()? == '\n' || self.peek()? == '\r' {
                                break;
                            }
                            self.consume()?;
                        }
                        let whitespace_count = self.current - self.start;
                        TokenType::Whitespace(whitespace_count)
                    }
                }

                _ => {
                    let mut text = String::new();
                    while !self.is_at_end() {
                        let c = self.peek()?;
                        if c == '{' || c == '<' || c == '\n' {
                            break;
                        }
                        text.push(c);
                        self.consume()?;
                    }
                    TokenType::Text(text)
                }
            };

            let token = Token::new(token_type, self.line, Some(self.start));

            match self.peek_previous()? {
                '\n' => self.line += 1,
                '\r' => {
                    self.line += 1;
                    if self.peek()? == '\n' {
                        self.current += 1;
                    }
                }
                _ => {}
            }

            tokens.push(token);
        }

        // Add EOF token
        let eof_token = Token::new(TokenType::Eof, self.line, None);
        tokens.push(eof_token);

        Ok(tokens)
    }

    fn peek(&self) -> Result<char, LexerError> {
        self.peek_at(0)
    }

    fn peek_next(&self) -> Result<char, LexerError> {
        self.peek_at(1)
    }

    fn peek_previous(&self) -> Result<char, LexerError> {
        self.peek_at(-1)
    }

    #[allow(dead_code)]
    fn peek_until(&self, end: &str) -> bool {
        let mut index = self.current;
        let end_chars: Vec<char> = end.chars().collect();

        while index < self.chars.len() {
            if self.chars[index..].starts_with(&end_chars) {
                return true;
            }
            index += 1;
        }
        false
    }

    #[allow(clippy::cast_sign_loss)]
    fn peek_at(&self, offset: isize) -> Result<char, LexerError> {
        // Safely handle negative offsets
        let index = if offset < 0 {
            // Check if we would underflow
            if self.current < offset.unsigned_abs() {
                return Err(LexerError::AtBeginningOfSource);
            }
            self.current - offset.unsigned_abs()
        } else {
            // Safe addition since offset is positive
            self.current + (offset as usize)
        };

        self.item_at(index)
    }

    fn item_at(&self, index: usize) -> Result<char, LexerError> {
        if index >= self.source.len() {
            // Return a null character when past the end, a bit of a departure from
            // idiomatic Rust code, but makes writing the matching above and testing
            // much easier
            Ok('\0')
        } else {
            self.source
                .chars()
                .nth(index)
                .ok_or(LexerError::InvalidCharacterAccess)
        }
    }

    fn matches(&mut self, pattern: &str) -> bool {
        let mut i = self.current;
        for c in pattern.chars() {
            if i >= self.chars.len() || self.chars[i] != c {
                return false;
            }
            i += 1;
        }
        true
    }

    fn is_at_end(&self) -> bool {
        self.current >= self.source.len()
    }

    fn consume(&mut self) -> Result<char, LexerError> {
        if self.is_at_end() {
            return Err(LexerError::AtEndOfSource);
        }
        self.current += 1;
        self.peek_previous()
    }

    fn consume_n(&mut self, count: usize) -> Result<String, LexerError> {
        let start = self.current;
        for _ in 0..count {
            self.consume()?;
        }
        Ok(self.source[start..self.current].trim().to_string())
    }

    #[allow(dead_code)]
    fn consume_chars(&mut self, s: &str) -> Result<char, LexerError> {
        for c in s.chars() {
            if c != self.peek()? {
                return Err(LexerError::UnexpectedCharacter(c, self.line));
            }
            self.consume()?;
        }
        self.peek_previous()
    }

    fn consume_until(&mut self, s: &str) -> Result<String, LexerError> {
        let start = self.current;
        while !self.is_at_end() {
            if self.chars[self.current..self.chars.len()]
                .starts_with(s.chars().collect::<Vec<_>>().as_slice())
            {
                return Ok(self.source[start..self.current].trim().to_string());
            }
            self.consume()?;
        }
        Err(LexerError::UnexpectedEndOfInput)
    }
}

#[derive(Error, Debug)]
pub enum LexerError {
    #[error("empty token at line {0}")]
    EmptyToken(usize),

    #[error("unexpected character '{0}' at line {1}")]
    UnexpectedCharacter(char, usize),

    #[error("unexpected end of input")]
    UnexpectedEndOfInput,

    #[error("source is empty")]
    EmptySource,

    #[error("at beginning of source")]
    AtBeginningOfSource,

    #[error("at end of source")]
    AtEndOfSource,

    #[error("invalid character access")]
    InvalidCharacterAccess,

    #[error("unexpected token type '{0:?}'")]
    UnexpectedTokenType(TokenType),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize_html() {
        let source = r#"<div class="container" id="main" disabled></div>"#;
        let mut lexer = Lexer::new(source);
        let tokens = lexer.tokenize().unwrap();
        insta::assert_yaml_snapshot!(tokens);
    }

    #[test]
    fn test_tokenize_django_variable() {
        let source = "{{ user.name|default:\"Anonymous\"|title }}";
        let mut lexer = Lexer::new(source);
        let tokens = lexer.tokenize().unwrap();
        insta::assert_yaml_snapshot!(tokens);
    }

    #[test]
    fn test_tokenize_django_block() {
        let source = "{% if user.is_staff %}Admin{% else %}User{% endif %}";
        let mut lexer = Lexer::new(source);
        let tokens = lexer.tokenize().unwrap();
        insta::assert_yaml_snapshot!(tokens);
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
        let tokens = lexer.tokenize().unwrap();
        insta::assert_yaml_snapshot!(tokens);
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
        let tokens = lexer.tokenize().unwrap();
        insta::assert_yaml_snapshot!(tokens);
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
        let tokens = lexer.tokenize().unwrap();
        insta::assert_yaml_snapshot!(tokens);
    }

    #[test]
    fn test_tokenize_error_cases() {
        // Unterminated tokens
        assert!(Lexer::new("{{ user.name").tokenize().is_err()); // No closing }}
        assert!(Lexer::new("{% if").tokenize().is_err()); // No closing %}
        assert!(Lexer::new("{#").tokenize().is_err()); // No closing #}
        assert!(Lexer::new("<div").tokenize().is_err()); // No closing >

        // Invalid characters or syntax within tokens
        assert!(Lexer::new("{{}}").tokenize().is_ok()); // Empty but valid
        assert!(Lexer::new("{%  %}").tokenize().is_ok()); // Empty but valid
        assert!(Lexer::new("{##}").tokenize().is_ok()); // Empty but valid
    }

    #[test]
    fn test_tokenize_nested_delimiters() {
        let source = r"{{ user.name }}
{% if true %}
{# comment #}
<!-- html comment -->
<div>text</div>";
        assert!(Lexer::new(source).tokenize().is_ok());
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
        let tokens = lexer.tokenize().unwrap();
        insta::assert_yaml_snapshot!(tokens);
    }
}

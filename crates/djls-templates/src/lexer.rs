use serde::Serialize;
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

    pub fn tokenize(&mut self) -> (Vec<Token>, Vec<LexerError>) {
        let mut tokens = Vec::new();
        let mut errors = Vec::new();

        while !self.is_at_end() {
            self.start = self.current;

            let token_type = match self.peek_char() {
                '{' => match self.peek_next_char() {
                    '%' => self.consume_django_block(&mut errors),
                    '{' => self.consume_django_variable(&mut errors),
                    '#' => self.consume_django_comment(&mut errors),
                    _ => {
                        self.advance(); // consume '{'
                        TokenType::Text(String::from("{"))
                    }
                },

                c if c == '\n' || c == '\r' => {
                    self.advance(); // consume \r or \n
                    if c == '\r' && self.peek_char() == '\n' {
                        self.advance(); // consume \n of \r\n
                    }
                    self.line += 1;
                    TokenType::Newline
                }

                c if c.is_whitespace() => {
                    self.advance(); // consume first whitespace
                    while !self.is_at_end() && self.peek_char().is_whitespace() {
                        if self.peek_char() == '\n' || self.peek_char() == '\r' {
                            break;
                        }
                        self.advance();
                    }
                    let whitespace_count = self.current - self.start;
                    TokenType::Whitespace(whitespace_count)
                }

                _ => {
                    let mut text = String::new();
                    while !self.is_at_end() {
                        let c = self.peek_char();
                        if c == '{' || c == '\n' || c == '\r' {
                            break;
                        }
                        text.push(c);
                        self.advance();
                    }
                    TokenType::Text(text)
                }
            };

            let token = Token::new(token_type, self.line, Some(self.start));
            tokens.push(token);
        }

        // Add EOF token
        let eof_token = Token::new(TokenType::Eof, self.line, None);
        tokens.push(eof_token);

        (tokens, errors)
    }

    fn peek_char(&self) -> char {
        if self.current >= self.chars.len() {
            '\0'
        } else {
            self.chars[self.current]
        }
    }

    fn peek_next_char(&self) -> char {
        if self.current + 1 >= self.chars.len() {
            '\0'
        } else {
            self.chars[self.current + 1]
        }
    }

    fn advance(&mut self) -> char {
        if self.is_at_end() {
            '\0'
        } else {
            self.current += 1;
            self.chars[self.current - 1]
        }
    }

    fn consume_django_block(&mut self, errors: &mut Vec<LexerError>) -> TokenType {
        self.advance(); // consume '{'
        self.advance(); // consume '%'
        
        match self.consume_until_or_error("%}") {
            Ok(content) => {
                self.advance(); // consume '%'
                self.advance(); // consume '}'
                TokenType::DjangoBlock(content)
            }
            Err(malformed_content) => {
                errors.push(LexerError::UnterminatedComment { start: self.start });
                self.sync_to_next_django_delimiter();
                TokenType::Error(malformed_content)
            }
        }
    }

    fn consume_django_variable(&mut self, errors: &mut Vec<LexerError>) -> TokenType {
        self.advance(); // consume '{'
        self.advance(); // consume '{'
        
        match self.consume_until_or_error("}}") {
            Ok(content) => {
                self.advance(); // consume '}'
                self.advance(); // consume '}'
                TokenType::DjangoVariable(content)
            }
            Err(malformed_content) => {
                errors.push(LexerError::UnterminatedComment { start: self.start });
                self.sync_to_next_django_delimiter();
                TokenType::Error(malformed_content)
            }
        }
    }

    fn consume_django_comment(&mut self, errors: &mut Vec<LexerError>) -> TokenType {
        self.advance(); // consume '{'
        self.advance(); // consume '#'
        
        match self.consume_until_or_error("#}") {
            Ok(content) => {
                self.advance(); // consume '#'
                self.advance(); // consume '}'
                TokenType::Comment(content, "{#".to_string(), Some("#}".to_string()))
            }
            Err(malformed_content) => {
                errors.push(LexerError::UnterminatedComment { start: self.start });
                self.sync_to_next_django_delimiter();
                TokenType::Error(malformed_content)
            }
        }
    }

    fn consume_until_or_error(&mut self, delimiter: &str) -> Result<String, String> {
        let start = self.current;
        let delimiter_chars: Vec<char> = delimiter.chars().collect();
        
        while !self.is_at_end() {
            if self.current + delimiter_chars.len() <= self.chars.len() &&
               self.chars[self.current..self.current + delimiter_chars.len()] == delimiter_chars {
                let content = self.chars[start..self.current].iter().collect::<String>().trim().to_string();
                return Ok(content);
            }
            self.advance();
        }
        
        // Return the malformed content for error recovery
        let malformed_content = self.chars[start..self.current].iter().collect();
        Err(malformed_content)
    }

    fn sync_to_next_django_delimiter(&mut self) {
        while !self.is_at_end() {
            let c = self.peek_char();
            if c == '{' || c == '\n' || c == '\r' {
                break;
            }
            self.advance();
        }
    }

    fn peek(&self) -> Result<char, LexerError> {
        self.peek_at(0)
    }

    fn peek_previous(&self) -> Result<char, LexerError> {
        self.peek_at(-1)
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



    fn is_at_end(&self) -> bool {
        self.current >= self.source.len()
    }

    fn consume(&mut self) -> Result<char, LexerError> {
        if self.is_at_end() {
            return Err(LexerError::AtEndOfSource);
        }
        let c = self.chars[self.current];
        self.current += 1;
        Ok(c)
    }


}

#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize)]
pub enum LexerError {
    #[error("unterminated string starting at position {start}: {partial_content:?}")]
    UnterminatedString {
        start: usize,
        partial_content: String,
    },

    #[error("unterminated comment starting at position {start}")]
    UnterminatedComment { start: usize },

    #[error("malformed delimiter at position {position}: found '{found}', expected '{expected}'")]
    MalformedDelimiter {
        position: usize,
        found: String,
        expected: String,
    },

    #[error("invalid escape sequence at position {position}: '{sequence}'")]
    InvalidEscape { position: usize, sequence: String },

    #[error("unexpected character '{0}' at line {1}")]
    UnexpectedCharacter(char, usize),

    #[error("unexpected end of input")]
    UnexpectedEndOfInput,

    #[error("source is empty")]
    EmptySource,

    #[error("unexpected token type '{0:?}'")]
    UnexpectedTokenType(TokenType),

    // TODO: Remove these deprecated variants after updating all references
    #[error("empty token at line {0}")]
    EmptyToken(usize),

    #[error("at beginning of source")]
    AtBeginningOfSource,

    #[error("at end of source")]
    AtEndOfSource,

    #[error("invalid character access")]
    InvalidCharacterAccess,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize_html() {
        let source = r#"<div class="container" id="main" disabled></div>"#;
        let mut lexer = Lexer::new(source);
        let (tokens, _errors) = lexer.tokenize();
        insta::assert_yaml_snapshot!(tokens);
    }

    #[test]
    fn test_tokenize_django_variable() {
        let source = "{{ user.name|default:\"Anonymous\"|title }}";
        let mut lexer = Lexer::new(source);
        let (tokens, _errors) = lexer.tokenize();
        insta::assert_yaml_snapshot!(tokens);
    }

    #[test]
    fn test_tokenize_django_block() {
        let source = "{% if user.is_staff %}Admin{% else %}User{% endif %}";
        let mut lexer = Lexer::new(source);
        let (tokens, _errors) = lexer.tokenize();
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
        let (tokens, _errors) = lexer.tokenize();
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
        let (tokens, _errors) = lexer.tokenize();
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
        let (tokens, _errors) = lexer.tokenize();
        insta::assert_yaml_snapshot!(tokens);
    }

    #[test]
    fn test_tokenize_error_cases() {
        // Unterminated tokens should create Error tokens, not fail
        let (tokens, errors) = Lexer::new("{{ user.name").tokenize(); 
        assert!(!errors.is_empty()); // Should have errors
        assert!(tokens.iter().any(|t| matches!(t.token_type(), TokenType::Error(_))));

        let (tokens, errors) = Lexer::new("{% if").tokenize();
        assert!(!errors.is_empty()); // Should have errors
        assert!(tokens.iter().any(|t| matches!(t.token_type(), TokenType::Error(_))));

        let (tokens, errors) = Lexer::new("{#").tokenize();
        assert!(!errors.is_empty()); // Should have errors
        assert!(tokens.iter().any(|t| matches!(t.token_type(), TokenType::Error(_))));

        let (tokens, _errors) = Lexer::new("<div").tokenize(); // Now just Text
        assert!(tokens.iter().any(|t| matches!(t.token_type(), TokenType::Text(_))));

        // Valid empty tokens
        let (tokens, errors) = Lexer::new("{{}}").tokenize();
        assert!(errors.is_empty()); // Should be valid
        assert!(tokens.iter().any(|t| matches!(t.token_type(), TokenType::DjangoVariable(_))));

        let (tokens, errors) = Lexer::new("{%  %}").tokenize();
        assert!(errors.is_empty()); // Should be valid
        assert!(tokens.iter().any(|t| matches!(t.token_type(), TokenType::DjangoBlock(_))));

        let (tokens, errors) = Lexer::new("{##}").tokenize();
        assert!(errors.is_empty()); // Should be valid
        assert!(tokens.iter().any(|t| matches!(t.token_type(), TokenType::Comment(_, _, _))));
    }

    #[test]
    fn test_tokenize_nested_delimiters() {
        let source = r"{{ user.name }}
{% if true %}
{# comment #}
<!-- html comment -->
<div>text</div>";
        let (tokens, errors) = Lexer::new(source).tokenize();
        assert!(errors.is_empty()); // Should be valid
        assert!(!tokens.is_empty());
    }

    #[test]
    fn test_tokenize_django_error_recovery() {
        // Test unterminated Django constructs
        let source = "{{ user.name {% if condition %} {# comment";
        let mut lexer = Lexer::new(source);
        let (tokens, errors) = lexer.tokenize();
        
        // Should have Error tokens for malformed constructs
        let error_tokens: Vec<_> = tokens.iter()
            .filter(|t| matches!(t.token_type(), TokenType::Error(_)))
            .collect();
        assert!(!error_tokens.is_empty());
        assert!(!errors.is_empty());
        insta::assert_yaml_snapshot!(tokens);
    }

    #[test]
    fn test_tokenize_mixed_delimiters() {
        // Test mixed/invalid delimiter combinations
        let source = "{{% invalid %}} {{% also invalid #} {{# wrong closer %}";
        let mut lexer = Lexer::new(source);
        let (tokens, errors) = lexer.tokenize();
        
        // First part should be parsed as DjangoVariable with unusual content
        // Second part should be Error token due to unterminated construct
        assert!(tokens.iter().any(|t| matches!(t.token_type(), TokenType::DjangoVariable(_))));
        assert!(tokens.iter().any(|t| matches!(t.token_type(), TokenType::Error(_))));
        assert!(!errors.is_empty());
        insta::assert_yaml_snapshot!(tokens);
    }

    #[test]
    fn test_tokenize_stray_closers() {
        // Test standalone closing delimiters
        let source = "}} %} #} normal text {{ valid }}";
        let mut lexer = Lexer::new(source);
        let (tokens, _errors) = lexer.tokenize();
        
        // Stray closers should be treated as Text
        let text_tokens: Vec<_> = tokens.iter()
            .filter(|t| matches!(t.token_type(), TokenType::Text(_)))
            .collect();
        assert!(!text_tokens.is_empty());
        insta::assert_yaml_snapshot!(tokens);
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
        let (tokens, _errors) = lexer.tokenize();
        insta::assert_yaml_snapshot!(tokens);
    }
}

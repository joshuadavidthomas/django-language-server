use thiserror::Error;

use crate::ast::Ast;
use crate::ast::AstError;
use crate::ast::Node;
use crate::ast::Span;
use crate::lexer::LexerError;
use crate::tokens::Token;
use crate::tokens::TokenStream;
use crate::tokens::TokenType;

pub struct Parser {
    tokens: TokenStream,
    current: usize,
    errors: Vec<ParserError>,
}

impl Parser {
    #[must_use]
    pub fn new(tokens: TokenStream) -> Self {
        Self {
            tokens,
            current: 0,
            errors: Vec::new(),
        }
    }

    pub fn parse(&mut self) -> Result<(Ast, Vec<ParserError>), ParserError> {
        let mut ast = Ast::default();
        ast.set_line_offsets(&self.tokens);

        while !self.is_at_end() {
            match self.next_node() {
                Ok(node) => {
                    ast.add_node(node);
                }
                Err(err) => {
                    if !self.is_at_end() {
                        self.errors.push(err);
                        self.synchronize()?;
                    }
                }
            }
        }

        Ok((ast.clone(), std::mem::take(&mut self.errors)))
    }

    fn next_node(&mut self) -> Result<Node, ParserError> {
        let token = self.consume()?;

        match token.token_type() {
            TokenType::Comment(_, open, _) => self.parse_comment(open),
            TokenType::Eof => Err(ParserError::stream_error(StreamError::AtEnd)),
            TokenType::DjangoBlock(_) => self.parse_django_block(),
            TokenType::DjangoVariable(_) => self.parse_django_variable(),
            TokenType::HtmlTagClose(_)
            | TokenType::HtmlTagOpen(_)
            | TokenType::HtmlTagVoid(_)
            | TokenType::Newline
            | TokenType::ScriptTagClose(_)
            | TokenType::ScriptTagOpen(_)
            | TokenType::StyleTagClose(_)
            | TokenType::StyleTagOpen(_)
            | TokenType::Text(_)
            | TokenType::Whitespace(_) => self.parse_text(),
        }
    }

    fn parse_comment(&mut self, open: &str) -> Result<Node, ParserError> {
        // Only treat Django comments as Comment nodes
        if open != "{#" {
            return self.parse_text();
        }

        let token = self.peek_previous()?;

        Ok(Node::Comment {
            content: token.content(),
            span: Span::from(token),
        })
    }

    pub fn parse_django_block(&mut self) -> Result<Node, ParserError> {
        let token = self.peek_previous()?;

        let args: Vec<String> = token
            .content()
            .split_whitespace()
            .map(String::from)
            .collect();
        let name = args.first().ok_or(ParserError::EmptyTag)?.clone();
        let bits = args.into_iter().skip(1).collect();
        let span = Span::from(token);

        Ok(Node::Tag { name, bits, span })
    }

    fn parse_django_variable(&mut self) -> Result<Node, ParserError> {
        let token = self.peek_previous()?;

        let content = token.content();
        let bits: Vec<&str> = content.split('|').collect();
        let var = bits
            .first()
            .ok_or(ParserError::EmptyTag)?
            .trim()
            .to_string();
        let filters = bits
            .into_iter()
            .skip(1)
            .map(|s| s.trim().to_string())
            .collect();
        let span = Span::from(token);

        Ok(Node::Variable { var, filters, span })
    }

    fn parse_text(&mut self) -> Result<Node, ParserError> {
        let token = self.peek_previous()?;

        if token.token_type() == &TokenType::Newline {
            return self.next_node();
        }

        let mut text = token.lexeme();

        while let Ok(token) = self.peek() {
            match token.token_type() {
                TokenType::DjangoBlock(_)
                | TokenType::DjangoVariable(_)
                | TokenType::Comment(_, _, _)
                | TokenType::Newline
                | TokenType::Eof => break,
                _ => {
                    let token_text = token.lexeme();
                    text.push_str(&token_text);
                    self.consume()?;
                }
            }
        }

        let content = match text.trim() {
            "" => return self.next_node(),
            trimmed => trimmed.to_string(),
        };

        let start = token.start().unwrap_or(0);
        let offset = u32::try_from(text.find(content.as_str()).unwrap_or(0))
            .expect("Offset should fit in u32");
        let length = u32::try_from(content.len())
            .expect("Content length should fit in u32");
        let span = Span::new(start + offset, length);

        Ok(Node::Text { content, span })
    }

    fn peek(&self) -> Result<Token, ParserError> {
        self.peek_at(0)
    }

    #[allow(dead_code)]
    fn peek_next(&self) -> Result<Token, ParserError> {
        self.peek_at(1)
    }

    fn peek_previous(&self) -> Result<Token, ParserError> {
        self.peek_at(-1)
    }

    #[allow(clippy::cast_sign_loss)]
    fn peek_at(&self, offset: isize) -> Result<Token, ParserError> {
        // Safely handle negative offsets
        let index = if offset < 0 {
            // Check if we would underflow
            if self.current < offset.unsigned_abs() {
                return Err(ParserError::stream_error(StreamError::BeforeStart));
            }
            self.current - offset.unsigned_abs()
        } else {
            // Safe addition since offset is positive
            self.current + (offset as usize)
        };
        
        self.item_at(index)
    }

    fn item_at(&self, index: usize) -> Result<Token, ParserError> {
        if let Some(token) = self.tokens.get(index) {
            Ok(token.clone())
        } else {
            let error = if self.tokens.is_empty() {
                ParserError::stream_error(StreamError::Empty)
            } else if index < self.current {
                ParserError::stream_error(StreamError::AtBeginning)
            } else if index >= self.tokens.len() {
                ParserError::stream_error(StreamError::AtEnd)
            } else {
                ParserError::stream_error(StreamError::InvalidAccess)
            };
            Err(error)
        }
    }

    fn is_at_end(&self) -> bool {
        self.current + 1 >= self.tokens.len()
    }

    fn consume(&mut self) -> Result<Token, ParserError> {
        if self.is_at_end() {
            return Err(ParserError::stream_error(StreamError::AtEnd));
        }
        self.current += 1;
        self.peek_previous()
    }

    #[allow(dead_code)]
    fn backtrack(&mut self, steps: usize) -> Result<Token, ParserError> {
        if self.current < steps {
            return Err(ParserError::stream_error(StreamError::AtBeginning));
        }
        self.current -= steps;
        self.peek_next()
    }

    fn synchronize(&mut self) -> Result<(), ParserError> {
        let sync_types = &[
            TokenType::DjangoBlock(String::new()),
            TokenType::DjangoVariable(String::new()),
            TokenType::Comment(String::new(), String::from("{#"), Some(String::from("#}"))),
            TokenType::Eof,
        ];

        while !self.is_at_end() {
            let current = self.peek()?;
            for sync_type in sync_types {
                if *current.token_type() == *sync_type {
                    return Ok(());
                }
            }
            self.consume()?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum StreamError {
    AtBeginning,
    BeforeStart,  // Added to match the usage in peek_at
    AtEnd,
    Empty,
    InvalidAccess,
}

#[derive(Debug, Error)]
pub enum ParserError {
    #[error("Unexpected token: expected {expected:?}, found {found} at position {position}")]
    UnexpectedToken {
        expected: Vec<String>,
        found: String,
        position: usize,
    },
    #[error("Invalid syntax: {context}")]
    InvalidSyntax { context: String },
    #[error("Empty tag")]
    EmptyTag,
    #[error("Lexer error: {0}")]
    Lexer(#[from] LexerError),
    #[error("Stream error: {kind:?}")]
    StreamError { kind: StreamError },
    #[error("AST error: {0}")]
    Ast(#[from] AstError),
}

impl ParserError {
    pub fn stream_error(kind: impl Into<StreamError>) -> Self {
        Self::StreamError { kind: kind.into() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;

    mod html {
        use super::*;
        #[test]
        fn test_parse_html_doctype() {
            let source = "<!DOCTYPE html>";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (nodelist, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(nodelist);
            assert!(errors.is_empty());
        }

        #[test]
        fn test_parse_html_tag() {
            let source = "<div class=\"container\">Hello</div>";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (nodelist, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(nodelist);
            assert!(errors.is_empty());
        }

        #[test]
        fn test_parse_html_void() {
            let source = "<input type=\"text\" />";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (nodelist, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(nodelist);
            assert!(errors.is_empty());
        }
    }

    mod django {
        use super::*;

        #[test]
        fn test_parse_django_variable() {
            let source = "{{ user.name }}";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (nodelist, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(nodelist);
            assert!(errors.is_empty());
        }

        #[test]
        fn test_parse_django_variable_with_filter() {
            let source = "{{ user.name|title }}";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (nodelist, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(nodelist);
            assert!(errors.is_empty());
        }

        #[test]
        fn test_parse_filter_chains() {
            let source = "{{ value|default:'nothing'|title|upper }}";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (nodelist, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(nodelist);
            assert!(errors.is_empty());
        }

        #[test]
        fn test_parse_django_if_block() {
            let source = "{% if user.is_authenticated %}Welcome{% endif %}";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (nodelist, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(nodelist);
            assert!(errors.is_empty());
        }

        #[test]
        fn test_parse_django_for_block() {
            let source = "{% for item in items %}{{ item }}{% empty %}No items{% endfor %}";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (nodelist, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(nodelist);
            assert!(errors.is_empty());
        }

        #[test]
        fn test_parse_complex_if_elif() {
            let source = "{% if x > 0 %}Positive{% elif x < 0 %}Negative{% else %}Zero{% endif %}";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (nodelist, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(nodelist);
            assert!(errors.is_empty());
        }

        #[test]
        fn test_parse_django_tag_assignment() {
            let source = "{% url 'view-name' as view %}";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (nodelist, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(nodelist);
            assert!(errors.is_empty());
        }

        #[test]
        fn test_parse_nested_for_if() {
            let source =
                "{% for item in items %}{% if item.active %}{{ item.name }}{% endif %}{% endfor %}";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (nodelist, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(nodelist);
            assert!(errors.is_empty());
        }

        #[test]
        fn test_parse_mixed_content() {
            let source = "Welcome, {% if user.is_authenticated %}
    {{ user.name|title|default:'Guest' }}
    {% for group in user.groups %}
        {% if forloop.first %}({% endif %}
        {{ group.name }}
        {% if not forloop.last %}, {% endif %}
        {% if forloop.last %}){% endif %}
    {% empty %}
        (no groups)
    {% endfor %}
{% else %}
    Guest
{% endif %}!";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (nodelist, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(nodelist);
            assert!(errors.is_empty());
        }
    }

    mod script {
        use super::*;

        #[test]
        fn test_parse_script() {
            let source = r#"<script type="text/javascript">
    // Single line comment
    const x = 1;
    /* Multi-line
        comment */
    console.log(x);
</script>"#;
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (nodelist, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(nodelist);
            assert!(errors.is_empty());
        }
    }

    mod style {
        use super::*;

        #[test]
        fn test_parse_style() {
            let source = r#"<style type="text/css">
    /* Header styles */
    .header {
        color: blue;
    }
</style>"#;
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (nodelist, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(nodelist);
            assert!(errors.is_empty());
        }
    }

    mod comments {
        use super::*;

        #[test]
        fn test_parse_comments() {
            let source = "<!-- HTML comment -->{# Django comment #}";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (nodelist, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(nodelist);
            assert!(errors.is_empty());
        }
    }

    mod whitespace {
        use super::*;

        #[test]
        fn test_parse_with_leading_whitespace() {
            let source = "     hello";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (nodelist, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(nodelist);
            assert!(errors.is_empty());
        }

        #[test]
        fn test_parse_with_leading_whitespace_newline() {
            let source = "\n     hello";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (nodelist, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(nodelist);
            assert!(errors.is_empty());
        }

        #[test]
        fn test_parse_with_trailing_whitespace() {
            let source = "hello     ";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (nodelist, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(nodelist);
            assert!(errors.is_empty());
        }

        #[test]
        fn test_parse_with_trailing_whitespace_newline() {
            let source = "hello     \n";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (nodelist, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(nodelist);
            eprintln!("{errors:?}");
            assert!(errors.is_empty());
        }
    }

    mod errors {
        use super::*;

        #[test]
        fn test_parse_unclosed_html_tag() {
            let source = "<div>";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (nodelist, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(nodelist);
            assert!(errors.is_empty());
        }

        #[test]
        fn test_parse_unclosed_django_if() {
            let source = "{% if user.is_authenticated %}Welcome";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (nodelist, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(nodelist);
            assert!(errors.is_empty()); // Parser doesn't care about semantics at this point
        }

        #[test]
        fn test_parse_unclosed_django_for() {
            let source = "{% for item in items %}{{ item.name }}";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (nodelist, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(nodelist);
            assert!(errors.is_empty()); // Parser doesn't care about semantics at this point
        }

        #[test]
        fn test_parse_unclosed_script() {
            let source = "<script>console.log('test');";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (nodelist, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(nodelist);
            assert!(errors.is_empty());
        }

        #[test]
        fn test_parse_unclosed_style() {
            let source = "<style>body { color: blue; ";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (nodelist, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(nodelist);
            assert!(errors.is_empty());
        }

        #[test]
        fn test_parse_error_recovery() {
            let source = r#"<div class="container">
    <h1>Header</h1>
    {% %}
        {# This if is unclosed which does matter #}
        <p>Welcome {{ user.name }}</p>
        <div>
            {# This div is unclosed which doesn't matter #}
        {% for item in items %}
            <span>{{ item }}</span>
        {% endfor %}
    <footer>Page Footer</footer>
</div>"#;
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (nodelist, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(nodelist);
            assert_eq!(errors.len(), 1);
            assert!(matches!(&errors[0], ParserError::EmptyTag));
        }
    }

    mod full_templates {
        use super::*;

        #[test]
        fn test_parse_full() {
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
                <h1>Welcome, {{ user.name|title|default:'Guest' }}!</h1>
                {% if user.is_staff %}
                    <span>Admin</span>
                {% else %}
                    <span>User</span>
                {% endif %}
            {% endif %}
        </div>
    </body>
</html>"#;
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (nodelist, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(nodelist);
            assert!(errors.is_empty());
        }
    }

    mod line_tracking {
        use super::*;

        #[test]
        fn test_parser_tracks_line_offsets() {
            let source = "line1\nline2";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (nodelist, errors) = parser.parse().unwrap();

            let offsets = nodelist.line_offsets();
            eprintln!("{offsets:?}");
            assert_eq!(offsets.position_to_line_col(0), (1, 0)); // Start of line 1
            assert_eq!(offsets.position_to_line_col(6), (2, 0)); // Start of line 2
            assert!(errors.is_empty());
        }
    }
}

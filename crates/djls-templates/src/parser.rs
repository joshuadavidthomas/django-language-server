use djls_source::Span;
use serde::Serialize;
use thiserror::Error;

use crate::nodelist::Node;
use crate::tokens::Token;

pub struct Parser {
    tokens: Vec<Token>,
    current: usize,
}

impl Parser {
    #[must_use]
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, current: 0 }
    }

    pub fn parse(&mut self) -> (Vec<Node>, Vec<ParseError>) {
        let mut nodelist = Vec::with_capacity(self.tokens.len() / 2);
        let mut errors = Vec::with_capacity(4);

        while !self.is_at_end() {
            match self.next_node() {
                Ok(node) => {
                    nodelist.push(node);
                }
                Err(error) => {
                    let (span, full_span) = self
                        .peek_previous()
                        .ok()
                        .or_else(|| self.peek().ok())
                        .map_or(
                            {
                                let empty = Span::new(0, 0);
                                (empty, empty)
                            },
                            super::tokens::Token::spans,
                        );

                    errors.push(error.clone());

                    nodelist.push(Node::Error {
                        span,
                        full_span,
                        error,
                    });

                    if !self.is_at_end() {
                        // Continue parsing even if synchronization fails
                        let _ = self.synchronize();
                    }
                }
            }
        }

        (nodelist, errors)
    }

    fn next_node(&mut self) -> Result<Node, ParseError> {
        let token = self.consume()?;

        match token {
            Token::Block { .. } => self.parse_block(),
            Token::Comment { .. } => self.parse_comment(),
            Token::Eof => Err(ParseError::stream_error(StreamError::AtEnd)),
            Token::Error { .. } => self.parse_error(),
            Token::Newline { .. } | Token::Text { .. } | Token::Whitespace { .. } => {
                self.parse_text()
            }
            Token::Variable { .. } => self.parse_variable(),
        }
    }

    pub fn parse_block(&mut self) -> Result<Node, ParseError> {
        let token = self.peek_previous()?;

        let Token::Block {
            content: content_ref,
            ..
        } = token
        else {
            return Err(ParseError::InvalidSyntax {
                context: "Expected Block token".to_string(),
            });
        };

        let mut parts = content_ref.split_ascii_whitespace();

        let name = parts.next().ok_or(ParseError::EmptyTag)?.to_string();

        let mut bits = Vec::with_capacity(parts.clone().count());
        for part in parts {
            bits.push(part.to_owned());
        }

        let span = token.content_span_or_fallback();

        Ok(Node::Tag { name, bits, span })
    }

    fn parse_comment(&mut self) -> Result<Node, ParseError> {
        let token = self.peek_previous()?;

        let span = token.content_span_or_fallback();
        Ok(Node::Comment {
            content: token.content(),
            span,
        })
    }

    fn parse_error(&mut self) -> Result<Node, ParseError> {
        let token = self.peek_previous()?;

        match token {
            Token::Error { content, span, .. } => {
                let error_text = content.clone();
                let full_span = token.full_span().unwrap_or(*span);
                Err(ParseError::MalformedConstruct {
                    position: full_span.start_usize(),
                    content: error_text,
                })
            }
            _ => Err(ParseError::InvalidSyntax {
                context: "Expected Error token".to_string(),
            }),
        }
    }

    fn parse_text(&mut self) -> Result<Node, ParseError> {
        let first_span = self.peek_previous()?.full_span_or_fallback();
        let start = first_span.start();
        let mut end = first_span.end();

        while let Ok(token) = self.peek() {
            match token {
                Token::Block { .. }
                | Token::Variable { .. }
                | Token::Comment { .. }
                | Token::Error { .. }
                | Token::Eof => break, // Stop at Django constructs, errors, or EOF
                Token::Text { .. } | Token::Whitespace { .. } | Token::Newline { .. } => {
                    // Update end position
                    let token_end = token.full_span_or_fallback().end();
                    end = end.max(token_end);
                    self.consume()?;
                }
            }
        }

        let length = end.saturating_sub(start);
        let span = Span::new(start, length);

        Ok(Node::Text { span })
    }

    fn parse_variable(&mut self) -> Result<Node, ParseError> {
        let token = self.peek_previous()?;

        let Token::Variable {
            content: content_ref,
            ..
        } = token
        else {
            return Err(ParseError::InvalidSyntax {
                context: "Expected Variable token".to_string(),
            });
        };

        let mut parts = content_ref.split('|');

        let var = parts.next().ok_or(ParseError::EmptyTag)?.trim().to_string();

        let filters: Vec<String> = parts.map(|s| s.trim().to_string()).collect();
        let span = token.content_span_or_fallback();

        Ok(Node::Variable { var, filters, span })
    }

    #[inline]
    fn peek(&self) -> Result<&Token, ParseError> {
        self.tokens.get(self.current).ok_or_else(|| {
            if self.tokens.is_empty() {
                ParseError::stream_error(StreamError::Empty)
            } else {
                ParseError::stream_error(StreamError::AtEnd)
            }
        })
    }

    #[inline]
    fn peek_previous(&self) -> Result<&Token, ParseError> {
        if self.current == 0 {
            return Err(ParseError::stream_error(StreamError::BeforeStart));
        }
        self.tokens
            .get(self.current - 1)
            .ok_or_else(|| ParseError::stream_error(StreamError::InvalidAccess))
    }

    #[inline]
    fn is_at_end(&self) -> bool {
        self.current + 1 >= self.tokens.len()
    }

    #[inline]
    fn consume(&mut self) -> Result<&Token, ParseError> {
        if self.is_at_end() {
            return Err(ParseError::stream_error(StreamError::AtEnd));
        }
        self.current += 1;
        self.peek_previous()
    }

    fn synchronize(&mut self) -> Result<(), ParseError> {
        while !self.is_at_end() {
            let current = self.peek()?;
            match current {
                Token::Block { .. }
                | Token::Variable { .. }
                | Token::Comment { .. }
                | Token::Eof => {
                    return Ok(());
                }
                _ => {}
            }
            self.consume()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum StreamError {
    AtBeginning,
    BeforeStart,
    AtEnd,
    Empty,
    InvalidAccess,
}

#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize)]
pub enum ParseError {
    #[error("Unexpected token: expected {expected:?}, found {found} at position {position}")]
    UnexpectedToken {
        expected: Vec<String>,
        found: String,
        position: usize,
    },

    #[error("Missing condition in '{tag}' tag at position {position}")]
    MissingCondition { tag: String, position: usize },

    #[error("Missing iterator in 'for' tag at position {position}")]
    MissingIterator { position: usize },

    #[error("Malformed variable at position {position}: {content}")]
    MalformedVariable { position: usize, content: String },

    #[error("Invalid filter syntax at position {position}: {reason}")]
    InvalidFilterSyntax { position: usize, reason: String },

    #[error("Unclosed tag at position {opener}: expected '{expected_closer}'")]
    UnclosedTag {
        opener: usize,
        expected_closer: String,
    },

    #[error("Invalid syntax: {context}")]
    InvalidSyntax { context: String },

    #[error("Empty tag")]
    EmptyTag,

    #[error("Malformed Django construct at position {position}: {content}")]
    MalformedConstruct { position: usize, content: String },

    #[error("Stream error: {kind:?}")]
    StreamError { kind: StreamError },
}

impl ParseError {
    pub fn stream_error(kind: impl Into<StreamError>) -> Self {
        Self::StreamError { kind: kind.into() }
    }
}

#[cfg(test)]
mod tests {
    use serde::Serialize;

    use super::*;
    use crate::lexer::Lexer;

    fn parse_test_template(source: &str) -> Vec<Node> {
        let mut lexer = Lexer::new(source);
        let tokens = lexer.tokenize();
        let mut parser = Parser::new(tokens);
        let (nodes, _errors) = parser.parse();
        nodes
    }

    #[derive(Debug, Clone, PartialEq, Serialize)]
    struct TestNodeList {
        nodelist: Vec<TestNode>,
    }

    #[derive(Debug, Clone, PartialEq, Serialize)]
    #[serde(tag = "type")]
    enum TestNode {
        Tag {
            name: String,
            bits: Vec<String>,
            span: (u32, u32),
            full_span: (u32, u32),
        },
        Comment {
            content: String,
            span: (u32, u32),
            full_span: (u32, u32),
        },
        Text {
            span: (u32, u32),
            full_span: (u32, u32),
        },
        Variable {
            var: String,
            filters: Vec<String>,
            span: (u32, u32),
            full_span: (u32, u32),
        },
        Error {
            span: (u32, u32),
            full_span: (u32, u32),
            error: ParseError,
        },
    }

    impl TestNode {
        fn from_node(node: &Node) -> Self {
            match node {
                Node::Tag { name, bits, span } => TestNode::Tag {
                    name: name.clone(),
                    bits: bits.clone(),
                    span: span.into(),
                    full_span: node.full_span().into(),
                },
                Node::Comment { content, span } => TestNode::Comment {
                    content: content.clone(),
                    span: span.into(),
                    full_span: node.full_span().into(),
                },
                Node::Text { span } => TestNode::Text {
                    span: span.into(),
                    full_span: node.full_span().into(),
                },
                Node::Variable { var, filters, span } => TestNode::Variable {
                    var: var.clone(),
                    filters: filters.clone(),
                    span: span.into(),
                    full_span: node.full_span().into(),
                },
                Node::Error {
                    span,
                    full_span,
                    error,
                } => TestNode::Error {
                    span: span.into(),
                    full_span: full_span.into(),
                    error: error.clone(),
                },
            }
        }
    }

    fn convert_nodelist_for_testing(nodes: &[Node]) -> TestNodeList {
        TestNodeList {
            nodelist: nodes.iter().map(TestNode::from_node).collect(),
        }
    }

    mod html {
        use super::*;

        #[test]
        fn test_parse_html_doctype() {
            let source = "<!DOCTYPE html>";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_html_tag() {
            let source = "<div class=\"container\">Hello</div>";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_html_void() {
            let source = "<input type=\"text\" />";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }
    }

    mod django {
        use super::*;

        #[test]
        fn test_parse_django_variable() {
            let source = "{{ user.name }}";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_django_variable_with_filter() {
            let source = "{{ user.name|title }}";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_filter_chains() {
            let source = "{{ value|default:'nothing'|title|upper }}";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_django_if_block() {
            let source = "{% if user.is_authenticated %}Welcome{% endif %}";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_django_for_block() {
            let source =
                "{% for item in items %}{{ item }}{% empty %}No items{% endfor %}".to_string();
            let nodelist = parse_test_template(&source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_complex_if_elif() {
            let source = "{% if x > 0 %}Positive{% elif x < 0 %}Negative{% else %}Zero{% endif %}";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_django_tag_assignment() {
            let source = "{% url 'view-name' as view %}";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_nested_for_if() {
            let source =
                "{% for item in items %}{% if item.active %}{{ item.name }}{% endif %}{% endfor %}"
                    .to_string();
            let nodelist = parse_test_template(&source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
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
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
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
</script>"#
                .to_string();
            let nodelist = parse_test_template(&source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
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
</style>"#
                .to_string();
            let nodelist = parse_test_template(&source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }
    }

    mod comments {
        use super::*;

        #[test]
        fn test_parse_comments() {
            let source = "<!-- HTML comment -->{# Django comment #}";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }
    }

    mod whitespace {
        use super::*;

        #[test]
        fn test_parse_with_leading_whitespace() {
            let source = "     hello";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_with_leading_whitespace_newline() {
            let source = "\n     hello";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_with_trailing_whitespace() {
            let source = "hello     ";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_with_trailing_whitespace_newline() {
            let source = "hello     \n";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }
    }

    mod errors {
        use super::*;

        #[test]
        fn test_parse_unclosed_html_tag() {
            let source = "<div>";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_unclosed_django_if() {
            let source = "{% if user.is_authenticated %}Welcome";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_unclosed_django_for() {
            let source = "{% for item in items %}{{ item.name }}";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_unclosed_script() {
            let source = "<script>console.log('test');";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_unclosed_style() {
            let source = "<style>body { color: blue; ";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        #[test]
        fn test_parse_unclosed_variable_token() {
            let source = "{{ user";
            let nodelist = parse_test_template(source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }

        // TODO: fix this so we can test against errors returned by parsing
        // #[test]
        // fn test_parse_error_recovery() {
        //     let source = r#"<div class="container">
        //     <h1>Header</h1>
        //     {% %}
        //         {# This if is unclosed which does matter #}
        //         <p>Welcome {{ user.name }}</p>
        //         <div>
        //             {# This div is unclosed which doesn't matter #}
        //         {% for item in items %}
        //             <span>{{ item }}</span>
        //         {% endfor %}
        //     <footer>Page Footer</footer>
        // </div>"#;
        //     let tokens = Lexer::new(source).tokenize().unwrap();
        //     let mut parser = create_test_parser(tokens);
        //     let (nodelist, errors) = parser.parse().unwrap();
        //     let nodelist = convert_nodelist_for_testing(ast.nodelist(parser.db), parser.db);
        //     insta::assert_yaml_snapshot!(nodelist);
        //     assert_eq!(errors.len(), 1);
        //     assert!(matches!(&errors[0], ParserError::EmptyTag));
        // }
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
</html>"#
                .to_string();
            let nodelist = parse_test_template(&source);
            let test_nodelist = convert_nodelist_for_testing(&nodelist);
            insta::assert_yaml_snapshot!(test_nodelist);
        }
    }
}

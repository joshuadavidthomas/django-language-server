use crate::ast::{Assignment, Ast, AstError, Block, DjangoFilter, LineOffsets, Node, Span, Tag};
use crate::tagspecs::{TagSpec, TagType};
use crate::tokens::{Token, TokenStream, TokenType};
use thiserror::Error;

pub struct Parser {
    tokens: TokenStream,
    current: usize,
}

impl Parser {
    pub fn new(tokens: TokenStream) -> Self {
        Self { tokens, current: 0 }
    }

    pub fn parse(&mut self) -> Result<(Ast, Vec<AstError>), ParserError> {
        let mut ast = Ast::default();
        let mut line_offsets = LineOffsets::new();
        let mut all_errors = Vec::new();

        // First pass: collect line offsets
        for token in self.tokens.tokens() {
            if let TokenType::Newline = token.token_type() {
                if let Some(start) = token.start() {
                    line_offsets.add_line(start + 1);
                }
            }
        }

        // Reset current position
        self.current = 0;

        // Second pass: parse nodes
        while !self.is_at_end() {
            match self.next_node() {
                Ok((node, errors)) => {
                    ast.add_node(node);
                    all_errors.extend(errors);
                }
                Err(_) => self.synchronize()?,
            }
        }

        ast.set_line_offsets(line_offsets);
        Ok((ast, all_errors))
    }

    fn next_node(&mut self) -> Result<(Node, Vec<AstError>), ParserError> {
        if self.is_at_end() {
            return Err(ParserError::Ast(AstError::StreamError("AtEnd".to_string())));
        }

        let token = self.peek()?;
        match token.token_type() {
            TokenType::DjangoBlock(content) => {
                self.consume()?;
                self.parse_django_block(content)
            }
            TokenType::DjangoVariable(content) => {
                self.consume()?;
                Ok((self.parse_django_variable(content)?, vec![]))
            }
            TokenType::Text(_)
            | TokenType::Whitespace(_)
            | TokenType::Newline
            | TokenType::HtmlTagOpen(_)
            | TokenType::HtmlTagClose(_)
            | TokenType::HtmlTagVoid(_)
            | TokenType::ScriptTagOpen(_)
            | TokenType::ScriptTagClose(_)
            | TokenType::StyleTagOpen(_)
            | TokenType::StyleTagClose(_) => Ok((self.parse_text()?, vec![])),
            TokenType::Comment(content, start, end) => {
                self.consume()?;
                self.parse_comment(content, start, end.as_deref())
            }
            TokenType::Eof => Err(ParserError::Ast(AstError::StreamError("AtEnd".to_string()))),
        }
    }

    fn parse_django_block(&mut self, content: &str) -> Result<(Node, Vec<AstError>), ParserError> {
        let token = self.peek_previous()?;
        let start_pos = token.start().unwrap_or(0);
        let total_length = token.length().unwrap_or(0);
        let span = Span::new(start_pos, total_length);

        // Parse the tag name and any assignments
        let mut bits = content.split_whitespace();
        let tag_name = bits.next().unwrap_or_default().to_string();
        let bits_vec: Vec<String> = bits.map(|s| s.to_string()).collect();

        // Check for assignment syntax
        let mut assignments = Vec::new();
        let mut assignment = None;
        if bits_vec.len() > 2 && bits_vec[1] == "as" {
            assignment = Some(bits_vec[2].clone());
            assignments.push(Assignment {
                target: bits_vec[2].clone(),
                value: bits_vec[3..].join(" "),
            });
        }

        let tag = Tag {
            name: tag_name.clone(),
            bits: content.split_whitespace().map(|s| s.to_string()).collect(),
            span,
            tag_span: span,
            assignment,
        };

        // Check if this is a closing tag
        if tag_name.starts_with("end") {
            return Ok((Node::Block(Block::Closing { tag }), vec![]));
        }

        // Load tag specs
        let specs = TagSpec::load_builtin_specs()?;
        let spec = match specs.get(&tag_name) {
            Some(spec) => spec,
            None => return Ok((Node::Block(Block::Tag { tag }), vec![])),
        };

        match spec.tag_type {
            TagType::Block => {
                let mut nodes = Vec::new();
                let mut all_errors = Vec::new();

                // Parse child nodes until we find the closing tag
                while let Ok((node, errors)) = self.next_node() {
                    if let Node::Block(Block::Closing { tag: closing_tag }) = &node {
                        if let Some(expected_closing) = &spec.closing {
                            if closing_tag.name == *expected_closing {
                                return Ok((
                                    Node::Block(Block::Block {
                                        tag,
                                        nodes,
                                        closing: Some(Box::new(Block::Closing {
                                            tag: closing_tag.clone(),
                                        })),
                                        assignments: Some(assignments),
                                    }),
                                    all_errors,
                                ));
                            }
                        }
                    }
                    nodes.push(node);
                    all_errors.extend(errors);
                }

                // Add error for unclosed tag
                all_errors.push(AstError::UnclosedTag(tag_name.clone()));

                // Return the partial block with the error
                Ok((
                    Node::Block(Block::Block {
                        tag,
                        nodes,
                        closing: None,
                        assignments: Some(assignments),
                    }),
                    all_errors,
                ))
            }
            TagType::Tag => Ok((Node::Block(Block::Tag { tag }), vec![])),
            TagType::Variable => Ok((Node::Block(Block::Variable { tag }), vec![])),
            TagType::Inclusion => {
                let template_name = bits_vec.get(1).cloned().unwrap_or_default();
                Ok((Node::Block(Block::Inclusion { tag, template_name }), vec![]))
            }
        }
    }

    fn parse_django_variable(&mut self, content: &str) -> Result<Node, ParserError> {
        let token = self.peek_previous()?;
        let start = token.start().unwrap_or(0);

        let mut bits = Vec::new();
        let mut filters = Vec::new();

        let parts: Vec<&str> = content.split('|').map(|s| s.trim()).collect();
        if !parts.is_empty() {
            bits = parts[0].split('.').map(|s| s.trim().to_string()).collect();

            for filter_part in parts.iter().skip(1) {
                let filter_parts: Vec<&str> = filter_part.split(':').collect();
                let filter_name = filter_parts[0].trim();
                let filter_args = if filter_parts.len() > 1 {
                    filter_parts[1]
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .collect()
                } else {
                    Vec::new()
                };

                filters.push(DjangoFilter {
                    name: filter_name.to_string(),
                    args: filter_args,
                    span: Span::new(start + 4, content.len() as u32), // Account for {{ and space
                });
            }
        }

        Ok(Node::Variable {
            bits,
            filters,
            span: Span::new(start + 3, content.len() as u32), // Account for {{ and space
        })
    }

    fn parse_text(&mut self) -> Result<Node, ParserError> {
        let start_token = self.peek()?;
        let start_pos = start_token.start().unwrap_or(0);
        let total_length = start_token.length().unwrap_or(0);
        let span = Span::new(start_pos, total_length);

        let content = match start_token.token_type() {
            TokenType::Text(text) => text.to_string(),
            TokenType::Whitespace(count) => " ".repeat(*count),
            TokenType::Newline => "\n".to_string(),
            _ => {
                return Err(ParserError::Ast(AstError::InvalidTag(
                    "Expected text, whitespace, or newline token".to_string(),
                )))
            }
        };

        self.consume()?;

        Ok(Node::Text { content, span })
    }

    fn parse_comment(
        &mut self,
        content: &str,
        start: &str,
        end: Option<&str>,
    ) -> Result<(Node, Vec<AstError>), ParserError> {
        let start_token = self.peek_previous()?;
        let start_pos = start_token.start().unwrap_or(0);
        let total_length = (content.len() + start.len() + end.map_or(0, |e| e.len())) as u32;
        let span = Span::new(start_pos, total_length);
        Ok((
            Node::Comment {
                content: content.to_string(),
                span,
            },
            vec![],
        ))
    }

    fn peek(&self) -> Result<Token, ParserError> {
        self.peek_at(0)
    }

    fn peek_previous(&self) -> Result<Token, ParserError> {
        self.peek_at(-1)
    }

    fn peek_at(&self, offset: isize) -> Result<Token, ParserError> {
        let index = self.current as isize + offset;
        self.item_at(index as usize)
    }

    fn item_at(&self, index: usize) -> Result<Token, ParserError> {
        if let Some(token) = self.tokens.get(index) {
            Ok(token.clone())
        } else {
            let error = if self.tokens.is_empty() {
                ParserError::stream_error("Empty")
            } else if index < self.current {
                ParserError::stream_error("AtBeginning")
            } else if index >= self.tokens.len() {
                ParserError::stream_error("AtEnd")
            } else {
                ParserError::stream_error("InvalidAccess")
            };
            Err(error)
        }
    }

    fn is_at_end(&self) -> bool {
        self.current + 1 >= self.tokens.len()
    }

    fn consume(&mut self) -> Result<Token, ParserError> {
        if self.is_at_end() {
            return Err(ParserError::stream_error("AtEnd"));
        }
        self.current += 1;
        self.peek_previous()
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
pub enum Signal {
    ClosingTagFound(String),
    IntermediateTagFound(String, Vec<String>),
    IntermediateTag(String),
    SpecialTag(String),
    ClosingTag,
}

#[derive(Debug, Error)]
pub enum ParserError {
    #[error("{0}")]
    Ast(#[from] AstError),
    #[error("Signal: {0:?}")]
    ErrorSignal(Signal),
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

impl ParserError {
    pub fn stream_error(kind: impl Into<String>) -> Self {
        Self::Ast(AstError::StreamError(kind.into()))
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
            let (ast, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
            assert!(errors.is_empty());
        }
        #[test]
        fn test_parse_html_tag() {
            let source = "<div class=\"container\">Hello</div>";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (ast, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
            assert!(errors.is_empty());
        }
        #[test]
        fn test_parse_html_void() {
            let source = "<input type=\"text\" />";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (ast, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
            assert!(errors.is_empty());
        }
    }
    mod django {
        use super::*;
        #[test]
        fn test_parse_django_variable() {
            let source = "{{ user.name|title }}";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (ast, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
            assert!(errors.is_empty());
        }
        #[test]
        fn test_parse_filter_chains() {
            let source = "{{ value|default:'nothing'|title|upper }}";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (ast, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
            assert!(errors.is_empty());
        }
        #[test]
        fn test_parse_django_if_block() {
            let source = "{% if user.is_authenticated %}Welcome{% endif %}";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (ast, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
            assert!(errors.is_empty());
        }
        #[test]
        fn test_parse_django_for_block() {
            let source = "{% for item in items %}{{ item }}{% empty %}No items{% endfor %}";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (ast, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
            assert!(errors.is_empty());
        }
        #[test]
        fn test_parse_complex_if_elif() {
            let source = "{% if x > 0 %}Positive{% elif x < 0 %}Negative{% else %}Zero{% endif %}";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (ast, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
            assert!(errors.is_empty());
        }
        #[test]
        fn test_parse_nested_for_if() {
            let source =
                "{% for item in items %}{% if item.active %}{{ item.name }}{% endif %}{% endfor %}";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (ast, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
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
            let (ast, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
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
            let (ast, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
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
            let (ast, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
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
            let (ast, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
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
            let (ast, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
            assert!(errors.is_empty());
        }
        #[test]
        fn test_parse_unclosed_django_if() {
            let source = "{% if user.is_authenticated %}Welcome";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (ast, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
            assert_eq!(errors.len(), 1);
            assert!(matches!(&errors[0], AstError::UnclosedTag(tag) if tag == "if"));
        }
        #[test]
        fn test_parse_unclosed_django_for() {
            let source = "{% for item in items %}{{ item.name }}";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (ast, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
            assert_eq!(errors.len(), 1);
            assert!(matches!(&errors[0], AstError::UnclosedTag(tag) if tag == "for"));
        }
        #[test]
        fn test_parse_unclosed_script() {
            let source = "<script>console.log('test');";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (ast, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
            assert!(errors.is_empty());
        }
        #[test]
        fn test_parse_unclosed_style() {
            let source = "<style>body { color: blue; ";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (ast, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
            assert!(errors.is_empty());
        }
        #[test]
        fn test_parse_error_recovery() {
            let source = r#"<div class="container">
    <h1>Header</h1>
    {% if user.is_authenticated %}
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
            let (ast, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
            assert_eq!(errors.len(), 1);
            assert!(matches!(&errors[0], AstError::UnclosedTag(tag) if tag == "if"));
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
            let (ast, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
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
            let (ast, errors) = parser.parse().unwrap();

            let offsets = ast.line_offsets();
            eprintln!("{:?}", offsets);
            assert_eq!(offsets.position_to_line_col(0), (1, 0)); // Start of line 1
            assert_eq!(offsets.position_to_line_col(6), (2, 0)); // Start of line 2
            assert!(errors.is_empty());
        }
    }
}

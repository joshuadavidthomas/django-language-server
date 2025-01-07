use crate::ast::{Ast, AstError, Block, DjangoFilter, LineOffsets, Node, Span, Tag};
use crate::tagspecs::{TagSpec, TagSpecs, TagType};
use crate::tokens::{Token, TokenStream, TokenType};
use thiserror::Error;

pub struct Parser {
    tokens: TokenStream,
    current: usize,
    errors: Vec<ParserError>,
    tag_specs: TagSpecs, // Add TagSpecs field
}

impl Parser {
    // Modify the constructor to accept TagSpecs
    pub fn new(tokens: TokenStream, tag_specs: TagSpecs) -> Self {
        Self {
            tokens,
            current: 0,
            errors: Vec::new(),
            tag_specs,
        }
    }

impl Parser {
    pub fn new(tokens: TokenStream) -> Self {
        Self {
            tokens,
            current: 0,
            errors: Vec::new(),
        }
    }

    pub fn parse(&mut self) -> Result<(Ast, Vec<ParserError>), ParserError> {
        let mut ast = Ast::default();
        let mut line_offsets = LineOffsets::new();

        for token in self.tokens.tokens() {
            if let TokenType::Newline = token.token_type() {
                if let Some(start) = token.start() {
                    // Add offset for next line
                    line_offsets.add_line(start + 1);
                }
            }
        }

        self.current = 0;

        while !self.is_at_end() {
            match self.next_node() {
                Ok(node) => {
                    ast.add_node(node);
                }
                Err(_) => self.synchronize()?,
            }
        }

        ast.set_line_offsets(line_offsets);
        Ok((ast, std::mem::take(&mut self.errors)))
    }

    fn next_node(&mut self) -> Result<Node, ParserError> {
        let token = self.consume()?;

        match token.token_type() {
            TokenType::Comment(_, open, _) => self.parse_comment(open),
            TokenType::Eof => Err(ParserError::Ast(AstError::StreamError("AtEnd".to_string()))),
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
        };

        let token = self.peek_previous()?;

        Ok(Node::Comment {
            content: token.content(),
            span: Span::from(token),
        })
    }

    pub fn parse_django_block(&mut self) -> Result<Node, ParserError> {
        let token = self.peek_previous()?;

        let mut bits: Vec<String> = token
            .content()
            .split_whitespace()
            .map(String::from)
            .collect();
        let tag_name = bits.first().ok_or(ParserError::EmptyTag)?.clone();

        let span = Span::from(token);
        let tag_span = Span::new(*span.start(), tag_name.len() as u32);

        let assignment = if bits.len() >= 2 {
            let second_to_last_index = bits.len() - 2;
            if bits[second_to_last_index] == "as" {
                let value = bits.last().cloned();
                bits.truncate(bits.len() - 2);
                value
            } else {
                None
            }
        } else {
            None
        };

        let tag = Tag {
            name: tag_name.clone(),
            bits,
            span,
            tag_span,
            assignment,
        };

        let spec = self.tag_specs.get(&tag_name);

        match spec {
            Some(spec) => match spec.tag_type {
                TagType::Container => self.parse_block_tag(tag, spec),
                TagType::Single => Ok(Node::Block(Block::Single { tag })),
                TagType::Inclusion => {
                    let template_name = tag.bits.get(1).cloned().unwrap_or_default();
                    Ok(Node::Block(Block::Inclusion { tag, template_name }))
                }
            },

            None => Ok(Node::Block(Block::Single { tag })),
        }
    }

    fn parse_block_tag(&mut self, tag: Tag, spec: &TagSpec) -> Result<Node, ParserError> {
        let mut nodes = Vec::new();
        let mut closing = None;

        while !self.is_at_end() {
            match self.next_node() {
                Ok(Node::Block(Block::Single { tag: inner_tag })) => {
                    if self.is_closing_tag(&inner_tag, spec) {
                        closing = Some(Box::new(Block::Closing { tag: inner_tag }));
                        break;
                    } else if self.is_branch_tag(&inner_tag, spec) {
                        nodes.push(self.parse_branch_tag(inner_tag, spec)?);
                    } else {
                        nodes.push(Node::Block(Block::Single { tag: inner_tag }));
                    }
                }
                Ok(node) => nodes.push(node),
                Err(e) => {
                    self.errors.push(e);
                    break;
                }
            }
        }

        if spec.closing.is_some() && closing.is_none() {
            self.errors
                .push(ParserError::Ast(AstError::UnclosedTag(tag.name.clone())));
        }

        Ok(Node::Block(Block::Container {
            tag,
            nodes,
            closing,
        }))
    }

    fn parse_branch_tag(&mut self, branch_tag: Tag, spec: &TagSpec) -> Result<Node, ParserError> {
        let mut branch_nodes = Vec::new();

        while !self.is_at_end() {
            match self.next_node() {
                Ok(Node::Block(Block::Single { tag: inner_tag })) => {
                    if self.is_closing_tag(&inner_tag, spec) || self.is_branch_tag(&inner_tag, spec)
                    {
                        self.backtrack(1)?;
                        break;
                    } else {
                        branch_nodes.push(Node::Block(Block::Single { tag: inner_tag }));
                    }
                }
                Ok(node) => branch_nodes.push(node),
                Err(e) => {
                    self.errors.push(e);
                    break;
                }
            }
        }

        Ok(Node::Block(Block::Branch {
            tag: branch_tag,
            nodes: branch_nodes,
        }))
    }

    fn is_closing_tag(&self, tag: &Tag, spec: &TagSpec) -> bool {
        match &spec.closing {
            Some(expected_closing) => &tag.name == expected_closing,
            None => false,
        }
    }

    fn is_branch_tag(&self, tag: &Tag, spec: &TagSpec) -> bool {
        if let Some(branches) = &spec.branches {
            branches.contains(&tag.name)
        } else {
            false
        }
    }

    fn parse_django_variable(&mut self) -> Result<Node, ParserError> {
        let token = self.peek_previous()?;

        let content = token.content();
        let parts: Vec<&str> = content.split('|').collect();
        let bits: Vec<String> = parts[0].split('.').map(|s| s.trim().to_string()).collect();

        let mut filters = Vec::new();
        let mut filter_offset = parts[0].len() as u32 + 1;

        for filter_part in parts.iter().skip(1) {
            let filter_parts: Vec<&str> = filter_part.split(':').collect();
            let args = if filter_parts.len() > 1 {
                filter_parts[1]
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .collect()
            } else {
                Vec::new()
            };

            filters.push(DjangoFilter {
                name: filter_parts[0].trim().to_string(),
                args,
                span: Span::new(
                    token.start().unwrap_or(0) + filter_offset,
                    filter_part.len() as u32,
                ),
            });

            filter_offset += filter_part.len() as u32 + 1;
        }

        Ok(Node::Variable {
            bits,
            filters,
            span: Span::from(token),
        })
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
        let offset = text.find(content.as_str()).unwrap_or(0) as u32;
        let length = content.len() as u32;

        Ok(Node::Text {
            content,
            span: Span::new(start + offset, length),
        })
    }

    fn peek(&self) -> Result<Token, ParserError> {
        self.peek_at(0)
    }

    fn peek_next(&self) -> Result<Token, ParserError> {
        self.peek_at(1)
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

    fn backtrack(&mut self, steps: usize) -> Result<Token, ParserError> {
        if self.current < steps {
            return Err(ParserError::stream_error("AtBeginning"));
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
    #[error("empty tag")]
    EmptyTag,
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
            let source = "{{ user.name }}";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (ast, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
            assert!(errors.is_empty());
        }

        #[test]
        fn test_parse_django_variable_with_filter() {
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
        fn test_parse_django_tag_assignment() {
            let source = "{% url 'view-name' as view %}";
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

    mod whitespace {
        use super::*;

        #[test]
        fn test_parse_with_leading_whitespace() {
            let source = "     hello";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (ast, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
            assert!(errors.is_empty());
        }

        #[test]
        fn test_parse_with_leading_whitespace_newline() {
            let source = "\n     hello";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (ast, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
            assert!(errors.is_empty());
        }

        #[test]
        fn test_parse_with_trailing_whitespace() {
            let source = "hello     ";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (ast, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
            assert!(errors.is_empty());
        }

        #[test]
        fn test_parse_with_trailing_whitespace_newline() {
            let source = "hello     \n";
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
            assert!(
                matches!(&errors[0], ParserError::Ast(AstError::UnclosedTag(tag)) if tag == "if")
            );
        }

        #[test]
        fn test_parse_unclosed_django_for() {
            let source = "{% for item in items %}{{ item.name }}";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let (ast, errors) = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
            assert_eq!(errors.len(), 1);
            assert!(
                matches!(&errors[0], ParserError::Ast(AstError::UnclosedTag(tag)) if tag == "for")
            );
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
            assert!(
                matches!(&errors[0], ParserError::Ast(AstError::UnclosedTag(tag)) if tag == "if")
            );
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

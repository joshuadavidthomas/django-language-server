use crate::ast::{
    Ast, AstError, AttributeValue, DjangoFilter, DjangoNode, DjangoTagKind, HtmlNode, Node,
    ScriptCommentKind, ScriptNode, StyleNode,
};
use crate::tokens::{Token, TokenStream, TokenType};
use std::collections::BTreeMap;
use std::str::FromStr;
use thiserror::Error;

pub struct Parser {
    tokens: TokenStream,
    current: usize,
}

impl Parser {
    pub fn new(tokens: TokenStream) -> Self {
        Parser { tokens, current: 0 }
    }

    pub fn parse(&mut self) -> Result<Ast, ParserError> {
        let mut ast = Ast::default();

        while !self.is_at_end() {
            let node = self.next_node();
            match node {
                Ok(node) => {
                    ast.add_node(node);
                }
                Err(ParserError::AtEndOfStream) => {
                    if ast.nodes().is_empty() {
                        return Err(ParserError::UnexpectedEof);
                    }
                    break;
                }
                Err(_) => {
                    self.synchronize(&[
                        TokenType::DjangoBlock(String::new()),
                        TokenType::HtmlTagOpen(String::new()),
                        TokenType::HtmlTagVoid(String::new()),
                        TokenType::ScriptTagOpen(String::new()),
                        TokenType::StyleTagOpen(String::new()),
                        TokenType::Newline,
                        TokenType::Eof,
                    ])?;
                    continue;
                }
            }
        }

        Ok(ast.finalize()?)
    }

    fn next_node(&mut self) -> Result<Node, ParserError> {
        let token = self.peek()?;
        let node = match token.token_type() {
            TokenType::Comment(s, start, end) => self.parse_comment(s, start, end.as_deref()),
            TokenType::DjangoBlock(s) => self.parse_django_block(s),
            TokenType::DjangoVariable(s) => self.parse_django_variable(s),
            TokenType::Eof => self.parse_eof(),
            TokenType::HtmlTagClose(tag) => Err(ParserError::ClosingTagFound(tag.to_string())),
            TokenType::HtmlTagOpen(s) => self.parse_html_tag_open(s),
            TokenType::HtmlTagVoid(s) => self.parse_html_tag_void(s),
            TokenType::Newline => self.parse_newline(),
            TokenType::ScriptTagOpen(s) => self.parse_script_tag_open(s),
            TokenType::ScriptTagClose(_) => Err(ParserError::ClosingTagFound("script".to_string())),
            TokenType::StyleTagOpen(s) => self.parse_style_tag_open(s),
            TokenType::StyleTagClose(_) => Err(ParserError::ClosingTagFound("style".to_string())),
            TokenType::Text(s) => self.parse_text(s),
            TokenType::Whitespace(_) => self.parse_whitespace(),
        }?;
        Ok(node)
    }

    fn parse_comment(
        &mut self,
        content: &str,
        start: &str,
        end: Option<&str>,
    ) -> Result<Node, ParserError> {
        self.consume()?;

        match start {
            "{#" => Ok(Node::Django(DjangoNode::Comment(content.to_string()))),
            "<!--" => Ok(Node::Html(HtmlNode::Comment(content.to_string()))),
            "//" => Ok(Node::Script(ScriptNode::Comment {
                content: content.to_string(),
                kind: ScriptCommentKind::SingleLine,
            })),
            "/*" => {
                // Look back for script/style context
                let token_type = self
                    .peek_back(self.current)?
                    .iter()
                    .find_map(|token| match token.token_type() {
                        TokenType::ScriptTagOpen(_) => {
                            Some(TokenType::ScriptTagOpen(String::new()))
                        }
                        TokenType::StyleTagOpen(_) => Some(TokenType::StyleTagOpen(String::new())),
                        TokenType::ScriptTagClose(_) | TokenType::StyleTagClose(_) => None,
                        _ => None,
                    })
                    .ok_or(ParserError::InvalidMultLineComment)?;

                match token_type {
                    TokenType::ScriptTagOpen(_) => Ok(Node::Script(ScriptNode::Comment {
                        content: content.to_string(),
                        kind: ScriptCommentKind::MultiLine,
                    })),
                    TokenType::StyleTagOpen(_) => {
                        Ok(Node::Style(StyleNode::Comment(content.to_string())))
                    }
                    _ => unreachable!(),
                }
            }
            _ => Err(ParserError::UnexpectedToken(Token::new(
                TokenType::Comment(
                    content.to_string(),
                    start.to_string(),
                    end.map(String::from),
                ),
                0,
                None,
            ))),
        }
    }

    fn parse_django_block(&mut self, s: &str) -> Result<Node, ParserError> {
        self.consume()?;

        let bits: Vec<String> = s.split_whitespace().map(String::from).collect();
        let kind = DjangoTagKind::from_str(&bits[0])?;

        // If this is an end tag, signal it like we do with HTML closing tags
        if bits[0].starts_with("end") {
            return Err(ParserError::ClosingTagFound(bits[0].clone()));
        }

        let mut children = Vec::new();
        let end_tag = format!("end{}", bits[0]);

        while !self.is_at_end() {
            match self.next_node() {
                Ok(node) => {
                    println!("found django child node: {:?}", node);
                    children.push(node);
                }
                Err(ParserError::ClosingTagFound(tag)) => {
                    if tag == end_tag {
                        self.consume()?;
                        break;
                    }
                    // If it's not our end tag, keep collecting children
                }
                Err(e) => return Err(e),
            }
        }

        Ok(Node::Django(DjangoNode::Tag {
            kind,
            bits,
            children,
        }))
    }

    fn parse_django_variable(&mut self, s: &str) -> Result<Node, ParserError> {
        self.consume()?;

        let parts: Vec<&str> = s.split('|').collect();

        let bits: Vec<String> = parts[0].trim().split('.').map(String::from).collect();

        let filters: Vec<DjangoFilter> = parts[1..]
            .iter()
            .map(|filter_str| {
                let filter_parts: Vec<&str> = filter_str.trim().split(':').collect();
                let name = filter_parts[0].to_string();

                let arguments = if filter_parts.len() > 1 {
                    filter_parts[1]
                        .trim_matches('"')
                        .split(',')
                        .map(|arg| arg.trim().to_string())
                        .collect()
                } else {
                    Vec::new()
                };

                DjangoFilter::new(name, arguments)
            })
            .collect();

        Ok(Node::Django(DjangoNode::Variable { bits, filters }))
    }

    fn parse_eof(&mut self) -> Result<Node, ParserError> {
        if self.is_at_end() {
            self.consume()?;
            self.next_node()
        } else {
            Err(ParserError::UnexpectedEof)
        }
    }

    fn parse_html_tag_open(&mut self, s: &str) -> Result<Node, ParserError> {
        self.consume()?;

        let mut parts = s.split_whitespace();

        let tag_name = parts
            .next()
            .ok_or(ParserError::InvalidTokenAccess)?
            .to_string();

        let mut attributes = BTreeMap::new();

        for attr in parts {
            if let Some((key, value)) = attr.split_once('=') {
                // Key-value attribute (class="container")
                attributes.insert(
                    key.to_string(),
                    AttributeValue::Value(value.trim_matches('"').to_string()),
                );
            } else {
                // Boolean attribute (disabled)
                attributes.insert(attr.to_string(), AttributeValue::Boolean);
            }
        }

        let mut children = Vec::new();

        while !self.is_at_end() {
            match self.next_node() {
                Ok(node) => {
                    children.push(node);
                }
                Err(ParserError::ClosingTagFound(tag)) => {
                    if tag == tag_name {
                        self.consume()?;
                        break;
                    }
                }
                Err(e) => return Err(e),
            }
        }

        Ok(Node::Html(HtmlNode::Element {
            tag_name,
            attributes,
            children,
        }))
    }

    fn parse_html_tag_void(&mut self, s: &str) -> Result<Node, ParserError> {
        self.consume()?;
        let mut parts = s.split_whitespace();

        let tag_name = parts
            .next()
            .ok_or(ParserError::InvalidTokenAccess)?
            .to_string();

        let mut attributes = BTreeMap::new();

        for attr in parts {
            if let Some((key, value)) = attr.split_once('=') {
                attributes.insert(
                    key.to_string(),
                    AttributeValue::Value(value.trim_matches('"').to_string()),
                );
            } else {
                attributes.insert(attr.to_string(), AttributeValue::Boolean);
            }
        }

        Ok(Node::Html(HtmlNode::Void {
            tag_name,
            attributes,
        }))
    }

    fn parse_newline(&mut self) -> Result<Node, ParserError> {
        self.consume()?;
        self.next_node()
    }

    fn parse_script_tag_open(&mut self, s: &str) -> Result<Node, ParserError> {
        self.consume()?;

        let parts = s.split_whitespace();

        let mut attributes = BTreeMap::new();

        for attr in parts {
            if let Some((key, value)) = attr.split_once('=') {
                attributes.insert(
                    key.to_string(),
                    AttributeValue::Value(value.trim_matches('"').to_string()),
                );
            } else {
                attributes.insert(attr.to_string(), AttributeValue::Boolean);
            }
        }

        let mut children = Vec::new();

        while !self.is_at_end() {
            match self.next_node() {
                Ok(node) => {
                    children.push(node);
                }
                Err(ParserError::ClosingTagFound(tag)) => {
                    if tag == "script" {
                        self.consume()?;
                        break;
                    }
                    // If it's not our closing tag, keep collecting children
                }
                Err(e) => return Err(e),
            }
        }

        Ok(Node::Script(ScriptNode::Element {
            attributes,
            children,
        }))
    }

    fn parse_style_tag_open(&mut self, s: &str) -> Result<Node, ParserError> {
        self.consume()?;

        let mut parts = s.split_whitespace();

        let _tag_name = parts
            .next()
            .ok_or(ParserError::InvalidTokenAccess)?
            .to_string();

        let mut attributes = BTreeMap::new();

        for attr in parts {
            if let Some((key, value)) = attr.split_once('=') {
                attributes.insert(
                    key.to_string(),
                    AttributeValue::Value(value.trim_matches('"').to_string()),
                );
            } else {
                attributes.insert(attr.to_string(), AttributeValue::Boolean);
            }
        }

        let mut children = Vec::new();

        while !self.is_at_end() {
            match self.next_node() {
                Ok(node) => {
                    children.push(node);
                }
                Err(ParserError::ClosingTagFound(tag)) => {
                    if tag == "style" {
                        self.consume()?;
                        break;
                    }
                    // If it's not our closing tag, keep collecting children
                }
                Err(e) => return Err(e),
            }
        }

        Ok(Node::Style(StyleNode::Element {
            attributes,
            children,
        }))
    }

    fn parse_text(&mut self, s: &str) -> Result<Node, ParserError> {
        self.consume()?;

        Ok(Node::Text(s.to_string()))
    }

    fn parse_whitespace(&mut self) -> Result<Node, ParserError> {
        self.consume()?;
        self.next_node()
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

    fn peek_forward(&self, steps: usize) -> Result<Vec<Token>, ParserError> {
        (0..steps).map(|i| self.peek_at(i as isize)).collect()
    }

    fn peek_back(&self, steps: usize) -> Result<Vec<Token>, ParserError> {
        (1..=steps).map(|i| self.peek_at(-(i as isize))).collect()
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
                ParserError::EmptyTokenStream
            } else if index < self.current {
                ParserError::AtBeginningOfStream
            } else if index >= self.tokens.len() {
                ParserError::AtEndOfStream
            } else {
                ParserError::InvalidTokenAccess
            };
            Err(error)
        }
    }

    fn is_at_end(&self) -> bool {
        self.current + 1 >= self.tokens.len()
    }

    fn consume(&mut self) -> Result<Token, ParserError> {
        if self.is_at_end() {
            return Err(ParserError::AtEndOfStream);
        }
        self.current += 1;
        self.peek_previous()
    }

    fn backtrack(&mut self, steps: usize) -> Result<Token, ParserError> {
        if self.current < steps {
            return Err(ParserError::AtBeginningOfStream);
        }
        self.current -= steps;
        self.peek_next()
    }

    fn lookahead(&self, types: &[TokenType]) -> Result<bool, ParserError> {
        for (i, t) in types.iter().enumerate() {
            if !self.peek_at(i as isize)?.is_token_type(t) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn consume_if(&mut self, token_type: TokenType) -> Result<Token, ParserError> {
        let token = self.peek()?;
        if token.is_token_type(&token_type) {
            return Err(ParserError::ExpectedTokenType(
                self.peek()?.clone(),
                token_type,
            ));
        }
        self.consume()?;
        Ok(token)
    }

    fn consume_until(&mut self, end_type: TokenType) -> Result<Vec<Token>, ParserError> {
        let mut consumed = Vec::new();
        while !self.is_at_end() && self.peek()?.is_token_type(&end_type) {
            let token = self.consume()?;
            consumed.push(token);
        }
        Ok(consumed)
    }

    fn synchronize(&mut self, sync_types: &[TokenType]) -> Result<(), ParserError> {
        while !self.is_at_end() {
            if sync_types.contains(self.peek()?.token_type()) {
                return Ok(());
            }
            self.consume()?;
        }
        Ok(())
    }
}

#[derive(Error, Debug)]
pub enum ParserError {
    #[error("token stream is empty")]
    EmptyTokenStream,
    #[error("at beginning of token stream")]
    AtBeginningOfStream,
    #[error("at end of token stream")]
    AtEndOfStream,
    #[error("invalid token access")]
    InvalidTokenAccess,
    #[error("unexpected token '{0:?}', expected type '{1:?}'")]
    ExpectedTokenType(Token, TokenType),
    #[error("unexpected token '{0:?}'")]
    UnexpectedToken(Token),
    #[error("unexpected end tag: {0}")]
    UnexpectedEndTag(String),
    #[error("multi-line comment outside of script or style context")]
    InvalidMultLineComment,
    #[error("unexpected end of file")]
    UnexpectedEof,
    #[error("found closing tag: {0}")]
    ClosingTagFound(String),
    #[error(transparent)]
    Node(#[from] AstError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;

    #[test]
    fn test_parse_comments() {
        let source = r#"<!-- HTML comment -->
{# Django comment #}
<script>
    // JS single line
    /* JS multi
        line */
</script>
<style>
    /* CSS comment */
</style>"#;
        let tokens = Lexer::new(source).tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let ast = parser.parse().unwrap();
        insta::assert_yaml_snapshot!(ast);
    }

    #[test]
    fn test_parse_django_block() {
        let source = r#"{% if user.is_staff %}Admin{% else %}User{% endif %}"#;
        let tokens = Lexer::new(source).tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let ast = parser.parse().unwrap();
        insta::assert_yaml_snapshot!(ast);
    }

    #[test]
    fn test_parse_django_variable() {
        let source = r#"{{ user.name|default:"Anonymous"|title }}"#;
        let tokens = Lexer::new(source).tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let ast = parser.parse().unwrap();
        insta::assert_yaml_snapshot!(ast);
    }
    #[test]
    fn test_parse_html_tag() {
        let source = r#"<div class="container" id="main" disabled></div>"#;
        let tokens = Lexer::new(source).tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let ast = parser.parse().unwrap();
        insta::assert_yaml_snapshot!(ast);
    }

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
        let ast = parser.parse().unwrap();
        insta::assert_yaml_snapshot!(ast);
    }

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
        let ast = parser.parse().unwrap();
        insta::assert_yaml_snapshot!(ast);
    }

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
        let tokens = Lexer::new(source).tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let ast = parser.parse().unwrap();
        insta::assert_yaml_snapshot!(ast);
    }

    #[test]
    fn test_parse_unexpected_eof() {
        let source = "<div>\n";
        let tokens = Lexer::new(source).tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let ast = parser.parse();
        assert!(matches!(ast, Err(ParserError::UnexpectedEof)));
    }
}

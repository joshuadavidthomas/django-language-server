use crate::ast::{
    Ast, AstError, AttributeValue, DjangoFilter, DjangoNode, HtmlNode, Node, ScriptCommentKind,
    ScriptNode, StyleNode, TagNode,
};
use crate::tagspecs::TagSpec;
use crate::tokens::{Token, TokenStream, TokenType};
use std::collections::{BTreeMap, HashMap};
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
        let mut had_nodes = false;

        while !self.is_at_end() {
            match self.next_node() {
                Ok(node) => {
                    ast.add_node(node);
                    had_nodes = true;
                }
                Err(ParserError::StreamError(Stream::AtEnd)) => {
                    if !had_nodes {
                        return Err(ParserError::StreamError(Stream::UnexpectedEof));
                    }
                    break;
                }
                Err(ParserError::ErrorSignal(Signal::SpecialTag(tag))) => {
                    continue;
                }
                Err(ParserError::UnclosedTag(tag)) => {
                    return Err(ParserError::UnclosedTag(tag));
                }
                Err(e) => {
                    self.synchronize()?;
                    continue;
                }
            }
        }

        if !had_nodes {
            return Err(ParserError::StreamError(Stream::UnexpectedEof));
        }
        ast.finalize()?;
        Ok(ast)
    }

    fn next_node(&mut self) -> Result<Node, ParserError> {
        let token = self.consume()?;
        let node = match token.token_type() {
            TokenType::Comment(s, start, end) => self.parse_comment(s, start, end.as_deref()),
            TokenType::DjangoBlock(s) => self.parse_django_block(s),
            TokenType::DjangoVariable(s) => self.parse_django_variable(s),
            TokenType::Eof => {
                if self.is_at_end() {
                    self.next_node()
                } else {
                    Err(ParserError::StreamError(Stream::UnexpectedEof))
                }
            }
            TokenType::HtmlTagClose(tag) => {
                self.backtrack(1)?;
                Err(ParserError::ErrorSignal(Signal::ClosingTagFound(
                    tag.to_string(),
                )))
            }
            TokenType::HtmlTagOpen(s) => self.parse_html_tag_open(s),
            TokenType::HtmlTagVoid(s) => self.parse_html_tag_void(s),
            TokenType::Newline => self.next_node(),
            TokenType::ScriptTagClose(_) => {
                self.backtrack(1)?;
                Err(ParserError::ErrorSignal(Signal::ClosingTagFound(
                    "script".to_string(),
                )))
            }
            TokenType::ScriptTagOpen(s) => self.parse_script_tag_open(s),
            TokenType::StyleTagClose(_) => {
                self.backtrack(1)?;
                Err(ParserError::ErrorSignal(Signal::ClosingTagFound(
                    "style".to_string(),
                )))
            }
            TokenType::StyleTagOpen(s) => self.parse_style_tag_open(s),
            TokenType::Text(s) => Ok(Node::Text(s.to_string())),
            TokenType::Whitespace(_) => self.next_node(),
        }?;
        Ok(node)
    }

    fn parse_comment(
        &mut self,
        content: &str,
        start: &str,
        end: Option<&str>,
    ) -> Result<Node, ParserError> {
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
        let bits: Vec<String> = s.split_whitespace().map(String::from).collect();
        let tag_name = bits.first().ok_or(AstError::EmptyTag)?.clone();

        let specs = TagSpec::load_builtin_specs().unwrap_or_default();

        // Check if this is a closing tag according to ANY spec
        for (_, spec) in specs.iter() {
            if Some(&tag_name) == spec.closing.as_ref() {
                return Err(ParserError::ErrorSignal(Signal::SpecialTag(tag_name)));
            }
        }

        // Check if this is an intermediate tag according to ANY spec
        for (_, spec) in specs.iter() {
            if let Some(intermediates) = &spec.intermediates {
                if intermediates.contains(&tag_name) {
                    return Err(ParserError::ErrorSignal(Signal::SpecialTag(tag_name)));
                }
            }
        }

        // Get the tag spec for this tag
        let tag_spec = specs.get(tag_name.as_str()).cloned();

        let mut children = Vec::new();
        let mut branches = Vec::new();

        while !self.is_at_end() {
            match self.next_node() {
                Ok(node) => {
                    children.push(node);
                }
                Err(ParserError::ErrorSignal(Signal::SpecialTag(tag))) => {
                    if let Some(spec) = &tag_spec {
                        // Check if this is a closing tag
                        if Some(&tag) == spec.closing.as_ref() {
                            // Found our closing tag, create appropriate tag type
                            let tag_node = if !branches.is_empty() {
                                TagNode::Branching {
                                    name: tag_name,
                                    bits,
                                    children,
                                    branches,
                                }
                            } else {
                                TagNode::Block {
                                    name: tag_name,
                                    bits,
                                    children,
                                }
                            };
                            return Ok(Node::Django(DjangoNode::Tag(tag_node)));
                        }
                        // Check if this is an intermediate tag
                        if let Some(intermediates) = &spec.intermediates {
                            if intermediates.contains(&tag) {
                                // Add current children as a branch and start fresh
                                branches.push(TagNode::Block {
                                    name: tag.clone(),
                                    bits: vec![tag.clone()],
                                    children,
                                });
                                children = Vec::new();
                                continue;
                            }
                        }
                    }
                    // If we get here, it's an unexpected tag
                    return Err(ParserError::UnexpectedTag(tag));
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }

        // If we get here, we never found the closing tag
        Err(ParserError::UnclosedTag(tag_name))
    }

    fn parse_django_variable(&mut self, s: &str) -> Result<Node, ParserError> {
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

    fn parse_html_tag_open(&mut self, s: &str) -> Result<Node, ParserError> {
        let mut parts = s.split_whitespace();

        let tag_name = parts
            .next()
            .ok_or(ParserError::StreamError(Stream::InvalidAccess))?
            .to_string();

        if tag_name.to_lowercase() == "!doctype" {
            return Ok(Node::Html(HtmlNode::Doctype(tag_name)));
        }

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
                Err(ParserError::ErrorSignal(Signal::ClosingTagFound(tag))) => {
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
        let mut parts = s.split_whitespace();

        let tag_name = parts
            .next()
            .ok_or(ParserError::StreamError(Stream::InvalidAccess))?
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

    fn parse_script_tag_open(&mut self, s: &str) -> Result<Node, ParserError> {
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
                Err(ParserError::ErrorSignal(Signal::ClosingTagFound(tag))) => {
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
        let mut parts = s.split_whitespace();

        let _tag_name = parts
            .next()
            .ok_or(ParserError::StreamError(Stream::InvalidAccess))?
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
        let mut found_closing_tag = false;

        while !self.is_at_end() {
            match self.next_node() {
                Ok(node) => {
                    children.push(node);
                }
                Err(ParserError::ErrorSignal(Signal::ClosingTagFound(tag))) => {
                    if tag == "style" {
                        self.consume()?;
                        found_closing_tag = true;
                        break;
                    }
                    // If it's not our closing tag, keep collecting children
                }
                Err(e) => return Err(e),
            }
        }

        if !found_closing_tag {
            return Err(ParserError::UnclosedTag("style".to_string()));
        }

        Ok(Node::Style(StyleNode::Element {
            attributes,
            children,
        }))
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
                ParserError::StreamError(Stream::Empty)
            } else if index < self.current {
                ParserError::StreamError(Stream::AtBeginning)
            } else if index >= self.tokens.len() {
                ParserError::StreamError(Stream::AtEnd)
            } else {
                ParserError::StreamError(Stream::InvalidAccess)
            };
            Err(error)
        }
    }

    fn is_at_end(&self) -> bool {
        self.current + 1 >= self.tokens.len()
    }

    fn consume(&mut self) -> Result<Token, ParserError> {
        if self.is_at_end() {
            return Err(ParserError::StreamError(Stream::AtEnd));
        }
        self.current += 1;
        self.peek_previous()
    }

    fn backtrack(&mut self, steps: usize) -> Result<Token, ParserError> {
        if self.current < steps {
            return Err(ParserError::StreamError(Stream::AtBeginning));
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
        let token = self.consume()?;
        if token.token_type() == &token_type {
            Ok(token)
        } else {
            self.backtrack(1)?;
            Err(ParserError::ExpectedTokenType(format!("{:?}", token_type)))
        }
    }

    fn consume_until(&mut self, end_type: TokenType) -> Result<Vec<Token>, ParserError> {
        let mut consumed = Vec::new();
        while !self.is_at_end() && self.peek()?.is_token_type(&end_type) {
            let token = self.consume()?;
            consumed.push(token);
        }
        Ok(consumed)
    }

    fn synchronize(&mut self) -> Result<(), ParserError> {
        const SYNC_TYPES: &[TokenType] = &[
            TokenType::DjangoBlock(String::new()),
            TokenType::HtmlTagOpen(String::new()),
            TokenType::HtmlTagVoid(String::new()),
            TokenType::ScriptTagOpen(String::new()),
            TokenType::StyleTagOpen(String::new()),
            TokenType::Newline,
            TokenType::Eof,
        ];

        while !self.is_at_end() {
            let current = self.peek()?;

            for sync_type in SYNC_TYPES {
                if matches!(current.token_type(), sync_type) {
                    return Ok(());
                }
            }
            self.consume()?;
        }

        Ok(())
    }
}

#[derive(Error, Debug)]
pub enum ParserError {
    #[error("unclosed tag: {0}")]
    UnclosedTag(String),
    #[error("unexpected tag: {0}")]
    UnexpectedTag(String),
    #[error("unsupported tag type")]
    UnsupportedTagType,
    #[error("empty tag")]
    EmptyTag,
    #[error("invalid tag type")]
    InvalidTagType,
    #[error("missing required args")]
    MissingRequiredArgs,
    #[error("invalid argument '{0:?}' '{1:?}")]
    InvalidArgument(String, String),
    #[error("unexpected closing tag {0}")]
    UnexpectedClosingTag(String),
    #[error("unexpected intermediate tag {0}")]
    UnexpectedIntermediateTag(String),
    #[error("unclosed block {0}")]
    UnclosedBlock(String),
    #[error(transparent)]
    StreamError(#[from] Stream),
    #[error("internal signal: {0:?}")]
    ErrorSignal(Signal),
    #[error("expected token: {0}")]
    ExpectedTokenType(String),
    #[error("unexpected token '{0:?}'")]
    UnexpectedToken(Token),
    #[error("multi-line comment outside of script or style context")]
    InvalidMultLineComment,
    #[error(transparent)]
    AstError(#[from] AstError),
}

#[derive(Debug)]
pub enum Stream {
    Empty,
    AtBeginning,
    AtEnd,
    UnexpectedEof,
    InvalidAccess,
}

impl std::error::Error for Stream {}

impl std::fmt::Display for Stream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "is empty"),
            Self::AtBeginning => write!(f, "at beginning"),
            Self::AtEnd => write!(f, "at end"),
            Self::UnexpectedEof => write!(f, "unexpected end of file"),
            Self::InvalidAccess => write!(f, "invalid access"),
        }
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

#[cfg(test)]
mod tests {
    use super::Stream;
    use super::*;
    use crate::lexer::Lexer;

    mod html {
        use super::*;

        #[test]
        fn test_parse_html_doctype() {
            let source = "<!DOCTYPE html>";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let ast = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
        }

        #[test]
        fn test_parse_html_tag() {
            let source = "<div class=\"container\">Hello</div>";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let ast = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
        }

        #[test]
        fn test_parse_html_void() {
            let source = "<input type=\"text\" />";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let ast = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
        }
    }

    mod django {
        use super::*;

        #[test]
        fn test_parse_django_variable() {
            let source = "{{ user.name|title }}";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let ast = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
        }

        #[test]
        fn test_parse_filter_chains() {
            let source = "{{ value|default:'nothing'|title|upper }}";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let ast = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
        }

        #[test]
        fn test_parse_django_if_block() {
            let source = "{% if user.is_authenticated %}Welcome{% endif %}";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let ast = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
        }

        #[test]
        fn test_parse_django_for_block() {
            let source = "{% for item in items %}{{ item }}{% empty %}No items{% endfor %}";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let ast = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
        }

        #[test]
        fn test_parse_complex_if_elif() {
            let source = "{% if x > 0 %}Positive{% elif x < 0 %}Negative{% else %}Zero{% endif %}";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let ast = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
        }

        #[test]
        fn test_parse_nested_for_if() {
            let source =
                "{% for item in items %}{% if item.active %}{{ item.name }}{% endif %}{% endfor %}";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let ast = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
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
            let ast = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
        }
    }

    mod script {
        use super::*;

        #[test]
        fn test_parse_script() {
            let source = "<script>
                const x = 42;
                // JavaScript comment
                /* Multi-line
                   comment */
                console.log(x);
            </script>";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let ast = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
        }
    }

    mod style {
        use super::*;

        #[test]
        fn test_parse_style() {
            let source = "<style>
                /* CSS comment */
                body {
                    font-family: sans-serif;
                }
            </style>";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let ast = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
        }
    }

    mod comments {
        use super::*;

        #[test]
        fn test_parse_comments() {
            let source = "<!-- HTML comment -->{# Django comment #}";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let ast = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
        }
    }

    mod errors {
        use super::*;

        #[test]
        fn test_parse_unexpected_eof() {
            let source = "<div>\n";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let ast = parser.parse();
            assert!(matches!(
                ast,
                Err(ParserError::StreamError(Stream::UnexpectedEof))
            ));
        }

        #[test]
        fn test_parse_unclosed_django_if() {
            let source = "{% if user.is_authenticated %}Welcome";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let result = parser.parse();
            println!("Error: {:?}", result);
            assert!(matches!(result, Err(ParserError::UnclosedTag(tag)) if tag == "if"));
        }

        #[test]
        fn test_parse_unclosed_django_for() {
            let source = "{% for item in items %}{{ item.name }}";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let result = parser.parse();
            println!("Error: {:?}", result);
            assert!(matches!(result, Err(ParserError::UnclosedTag(tag)) if tag == "for"));
        }

        #[test]
        fn test_parse_unclosed_style() {
            let source = "<style>body { color: blue; ";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let result = parser.parse();
            println!("Error: {:?}", result);
            assert!(matches!(result, Err(ParserError::UnclosedTag(tag)) if tag == "style"));
        }
    }

    mod full_templates {
        use super::*;

        #[test]
        fn test_parse_full() {
            let source = r#"<!DOCTYPE html>
<html>
<head>
    <title>{% block title %}Default Title{% endblock %}</title>
    <style>
        /* CSS styles */
        body { font-family: sans-serif; }
    </style>
</head>
<body>
    <h1>Welcome{% if user.is_authenticated %}, {{ user.name }}{% endif %}!</h1>
    <script>
        // JavaScript code
        console.log('Hello!');
    </script>
</body>
</html>"#;
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let ast = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
        }
    }
}

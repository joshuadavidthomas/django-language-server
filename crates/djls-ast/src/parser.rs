use crate::ast::{
    Ast, AstError, AttributeValue, DjangoFilter, DjangoNode, DjangoTagKind, HtmlNode, Node,
    ScriptCommentKind, ScriptNode, StyleNode,
};
use crate::tokens::{Token, TokenStream, TokenType};
use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;
use thiserror::Error;

#[derive(Debug, Clone)]
struct TagSpec {
    tag_type: TagType,
    closing: Option<String>,
    intermediates: Option<Vec<String>>,
    valid_args: Option<Vec<String>>,
}
#[derive(Debug, Clone)]
enum TagType {
    Block,
}
pub struct Parser {
    tokens: TokenStream,
    current: usize,
    specs: HashMap<String, TagSpec>,
}

impl Parser {
    pub fn new(tokens: TokenStream) -> Self {
        Parser {
            tokens,
            current: 0,
            specs: get_tag_spec(),
        }
    }

    pub fn parse(&mut self) -> Result<Ast, ParserError> {
        let mut ast = Ast::default();

        while !self.is_at_end() {
            if let Some(node) = self.parse_next()? {
                ast.add_node(node);
            }
        }

        Ok(ast)
    }

    fn parse_next(&mut self) -> Result<Option<Node>, ParserError> {
        let token_type = self.peek()?.token_type().clone();
        match token_type {
            TokenType::DjangoBlock(content) => {
                let parts: Vec<_> = content.split_whitespace().collect();
                let tag_name = parts[0];

                let spec = if let Some(s) = self.specs.get(tag_name) {
                    s.clone()
                } else {
                    self.consume()?;
                    return Ok(None);
                };

                self.consume()?;
                match spec.tag_type {
                    TagType::Block => {
                        let mut children = Vec::new();

                        while !self.is_at_end() {
                            let inner_type = self.peek()?.token_type().clone();
                            match inner_type {
                                TokenType::DjangoBlock(inner) => {
                                    let inner_tag = inner.split_whitespace().next().unwrap();
                                    if inner_tag == spec.closing.as_ref().unwrap() {
                                        self.consume()?;
                                        break;
                                    } else if spec
                                        .intermediates
                                        .as_ref()
                                        .map_or(false, |i| i.contains(&inner_tag.to_string()))
                                    {
                                        children.push(Node::Django(DjangoNode::Tag {
                                            kind: DjangoTagKind::Other(inner_tag.to_string()),
                                            bits: vec![inner_tag.to_string()],
                                            children: vec![],
                                        }));
                                        self.consume()?;
                                    }
                                }
                                TokenType::Text(text) => {
                                    children.push(Node::Text(text));
                                    self.consume()?;
                                }
                                _ => break,
                            }
                        }

                        Ok(Some(Node::Django(DjangoNode::Tag {
                            kind: DjangoTagKind::from_str(tag_name)?,
                            bits: parts.iter().map(|s| s.to_string()).collect(),
                            children,
                        })))
                    }
                }
            }
            TokenType::Text(text) => {
                self.consume()?;
                Ok(Some(Node::Text(text)))
            }
            _ => Ok(None),
        }
    }

    fn peek(&self) -> Result<&Token, ParserError> {
        if self.is_at_end() {
            Err(ParserError::StreamError(Stream::UnexpectedEof))
        } else {
            Ok(&self.tokens[self.current])
        }
    }

    fn consume(&mut self) -> Result<(), ParserError> {
        if !self.is_at_end() {
            self.current += 1;
            Ok(())
        } else {
            Err(ParserError::StreamError(Stream::UnexpectedEof))
        }
    }

    fn is_at_end(&self) -> bool {
        self.current >= self.tokens.len()
            || matches!(self.tokens[self.current].token_type(), TokenType::Eof)
    }
}

fn get_tag_spec() -> HashMap<String, TagSpec> {
    let mut specs = HashMap::new();

    // If block
    specs.insert(
        "if".to_string(),
        TagSpec {
            tag_type: TagType::Block,
            closing: Some("endif".to_string()),
            intermediates: Some(vec!["else".to_string(), "elif".to_string()]),
            valid_args: Some(vec!["*".to_string()]),
        },
    );

    // For block
    specs.insert(
        "for".to_string(),
        TagSpec {
            tag_type: TagType::Block,
            closing: Some("endfor".to_string()),
            intermediates: Some(vec!["empty".to_string()]),
            introduces_vars: Some(vec!["forloop".to_string(), "{loop_var}".to_string()]),
            valid_args: Some(vec!["* in *".to_string()]),
        },
    );

    // With block
    specs.insert(
        "with".to_string(),
        TagSpec {
            tag_type: TagType::Block,
            closing: Some("endwith".to_string()),
            introduces_vars: Some(vec!["{var_name}".to_string()]),
            valid_args: Some(vec!["* as *".to_string()]),
        },
    );

    // Block tag
    specs.insert(
        "block".to_string(),
        TagSpec {
            tag_type: TagType::Block,
            closing: Some("endblock".to_string()),
            valid_args: Some(vec!["*".to_string()]),
        },
    );

    specs
}

#[derive(Error, Debug)]
pub enum ParserError {
    #[error("token stream {0}")]
    StreamError(Stream),
    #[error("parsing signal: {0:?}")]
    ErrorSignal(Signal),
    #[error("unexpected token, expected type '{0:?}'")]
    ExpectedTokenType(TokenType),
    #[error("unexpected token '{0:?}'")]
    UnexpectedToken(Token),
    #[error("multi-line comment outside of script or style context")]
    InvalidMultLineComment,
    #[error(transparent)]
    Ast(#[from] AstError),
}

#[derive(Debug)]
pub enum Stream {
    Empty,
    AtBeginning,
    AtEnd,
    UnexpectedEof,
    InvalidAccess,
}

#[derive(Debug)]
pub enum Signal {
    ClosingTagFound(String),
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;

    #[test]
    fn test_parse_django_block() {
        let source = r#"{% if user.is_staff %}Admin{% else %}User{% endif %}"#;
        let tokens = Lexer::new(source).tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let ast = parser.parse().unwrap();
        insta::assert_yaml_snapshot!(ast);
    }

    #[test]
    fn test_complex_django_blocks() {
        // Nested if blocks
        let source = r#"{% if user.is_staff %}
        {% if user.is_superuser %}
            Super Admin
        {% else %}
            Regular Admin
        {% endif %}
    {% else %}
        Not Staff
    {% endif %}"#;

        // For loop with empty and nested if
        let source2 = r#"{% for item in items %}
        {% if item.active %}
            {{ item.name }}
        {% endif %}
    {% empty %}
        No items found
    {% endfor %}"#;

        // Multiple elif blocks
        let source3 = r#"{% if user.is_superuser %}
        Super
    {% elif user.is_staff %}
        Staff
    {% elif user.is_authenticated %}
        User
    {% else %}
        Anonymous
    {% endif %}"#;

        // With block containing if
        let source4 = r#"{% with total=business.employees.count %}
        {% if total > 50 %}
            Large Business
        {% elif total > 10 %}
            Medium Business
        {% else %}
            Small Business
        {% endif %}
    {% endwith %}"#;

        // Block with filters
        let source5 = r#"{% block content %}
        {% if messages|length > 0 %}
            {% for message in messages %}
                {{ message|escape }}
            {% endfor %}
        {% endif %}
    {% endblock %}"#;

        let tests = [source, source2, source3, source4, source5];

        for (i, source) in tests.iter().enumerate() {
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let ast = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(format!("complex_django_{}", i), ast);
        }
    }
}

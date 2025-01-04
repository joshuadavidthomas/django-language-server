use crate::ast::{Ast, DjangoNode, DjangoTagKind, Node};
use crate::tagspecs::{TagSpec, TagType};
use crate::tokens::{Token, TokenStream, TokenType};

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
            if let Some(node) = self.parse_next()? {
                ast.add_node(node);
            }
        }

        Ok(ast)
    }
    fn parse_next(&mut self) -> Result<Option<Node>, ParserError> {
        let token = self.peek()?;
        self.consume()?;

        match token.token_type() {
            TokenType::DjangoBlock(s) => Ok(Some(self.parse_django_block(s)?)),
            TokenType::DjangoVariable(var) => Ok(Some(Node::Django(DjangoNode::Variable {
                bits: vec![var.trim().to_string()],
                filters: vec![],
            }))),
            TokenType::Text(text) => Ok(Some(Node::Text(text.to_string()))),
            TokenType::Eof => Ok(None),
            _ => Ok(None),
        }
    }

    fn parse_django_block(&mut self, s: &str) -> Result<Node, ParserError> {
        let parts: Vec<_> = s.split_whitespace().collect();
        let tag_name = parts[0];
        let full_path = format!("django.template.defaulttags.{}", tag_name);

        let specs = TagSpec::load_builtin_specs().unwrap_or_default();

        // Get spec info up front
        let (tag_type, closing_tag, intermediates) = if let Some(spec) = specs.get(&full_path) {
            (
                spec.tag_type.clone(),
                spec.closing.clone(),
                spec.intermediates.clone(),
            )
        } else {
            self.consume()?;
            return Ok(Node::Django(DjangoNode::Tag {
                kind: DjangoTagKind::from_str(tag_name, &full_path),
                bits: parts.iter().map(|s| s.to_string()).collect(),
                children: vec![],
            }));
        };

        self.consume()?;
        let bits = {
            let spec = specs.get(&full_path).unwrap(); // Safe because we checked above
            self.parse_tag_args(&parts, spec)?
        };

        match tag_type {
            TagType::Block => {
                let mut children = Vec::new();

                loop {
                    let next = self.peek()?;
                    match next.token_type().clone() {
                        TokenType::DjangoBlock(inner) => {
                            let inner_parts: Vec<_> = inner.split_whitespace().collect();
                            let inner_tag = inner_parts[0];

                            if Some(inner_tag.to_string()) == closing_tag {
                                self.consume()?;
                                break;
                            } else if intermediates
                                .as_ref()
                                .map_or(false, |i| i.contains(&inner_tag.to_string()))
                            {
                                self.consume()?;
                                children.push(Node::Django(DjangoNode::Tag {
                                    kind: DjangoTagKind::as_intermediate(
                                        inner_tag, &full_path, tag_name,
                                    ),
                                    bits: inner_parts.iter().map(|s| s.to_string()).collect(),
                                    children: vec![],
                                }));
                            } else if let Some(node) = self.parse_next()? {
                                children.push(node);
                            }
                        }
                        TokenType::DjangoVariable(var) => {
                            self.consume()?;
                            children.push(Node::Django(DjangoNode::Variable {
                                bits: vec![var.trim().to_string()],
                                filters: vec![],
                            }));
                        }
                        TokenType::Text(text) => {
                            self.consume()?;
                            children.push(Node::Text(text));
                        }
                        TokenType::Eof => break,
                        _ => {
                            self.consume()?;
                        }
                    }
                }

                Ok(Node::Django(DjangoNode::Tag {
                    kind: DjangoTagKind::from_str(tag_name, &full_path),
                    bits,
                    children,
                }))
            }
            _ => Ok(Node::Django(DjangoNode::Tag {
                kind: DjangoTagKind::from_str(tag_name, &full_path),
                bits,
                children: vec![],
            })),
        }
    }

    fn parse_tag_args(&self, parts: &[&str], spec: &TagSpec) -> Result<Vec<String>, ParserError> {
        // If no args specified in spec, just return all parts
        if spec.args.is_none() {
            return Ok(parts.iter().map(|s| s.to_string()).collect());
        }

        let args = spec.args.as_ref().unwrap();

        // Skip the tag name when checking args
        let arg_parts = &parts[1..];

        // Check we have enough args
        let required_count = args.iter().filter(|arg| arg.required).count();
        if arg_parts.len() < required_count {
            return Err(ParserError::NotEnoughArgs);
        }

        // For now, just return all parts as bits
        // We can add more validation later if needed
        Ok(parts.iter().map(|s| s.to_string()).collect())
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

#[derive(Debug)]
pub enum ParserError {
    StreamError(Stream),
    NotEnoughArgs,
    InvalidArg(String),
    MissingArgs,
}

#[derive(Debug)]
pub enum Stream {
    Empty,
    AtBeginning,
    AtEnd,
    UnexpectedEof,
    InvalidAccess,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use insta::assert_yaml_snapshot;

    fn create_token_stream(content: &str) -> TokenStream {
        let mut lexer = Lexer::new(content);
        lexer.tokenize().unwrap()
    }

    #[test]
    fn test_parse_if_tag() -> Result<(), ParserError> {
        let tokens = create_token_stream("{% if condition %}content{% endif %}");
        let mut parser = Parser::new(tokens);
        let ast = parser.parse()?;
        assert_yaml_snapshot!(ast);
        Ok(())
    }

    #[test]
    fn test_parse_if_else_tag() -> Result<(), ParserError> {
        let tokens =
            create_token_stream("{% if condition %}true content{% else %}false content{% endif %}");
        let mut parser = Parser::new(tokens);
        let ast = parser.parse()?;
        assert_yaml_snapshot!(ast);
        Ok(())
    }

    #[test]
    fn test_parse_for_tag() -> Result<(), ParserError> {
        let tokens = create_token_stream("{% for item in items %}{{ item }}{% endfor %}");
        let mut parser = Parser::new(tokens);
        let ast = parser.parse()?;
        assert_yaml_snapshot!(ast);
        Ok(())
    }

    #[test]
    fn test_parse_for_empty_tag() -> Result<(), ParserError> {
        let tokens = create_token_stream(
            "{% for item in items %}{{ item }}{% empty %}No items!{% endfor %}",
        );
        let mut parser = Parser::new(tokens);
        let ast = parser.parse()?;
        assert_yaml_snapshot!(ast);
        Ok(())
    }
}

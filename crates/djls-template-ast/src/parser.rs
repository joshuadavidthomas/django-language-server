use crate::ast::{Ast, AstError, BlockType, DjangoFilter, Node};
use crate::tagspecs::TagSpec;
use crate::tokens::{Token, TokenStream, TokenType};
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
            match self.next_node() {
                Ok(node) => {
                    ast.add_node(node);
                }
                Err(ParserError::ErrorSignal(Signal::SpecialTag(_))) => {
                    continue;
                }
                Err(err) => {
                    match err {
                        ParserError::Ast(err, Some(node)) => {
                            ast.add_node(node);
                            ast.add_error(err);
                        }
                        ParserError::Ast(err, None) => {
                            ast.add_error(err);
                        }
                        _ => return Err(err),
                    }

                    if let Err(e) = self.synchronize() {
                        match e {
                            ParserError::Ast(AstError::StreamError(ref kind), _)
                                if kind == "AtEnd" =>
                            {
                                break
                            }
                            _ => return Err(e),
                        }
                    }
                    continue;
                }
            }
        }

        ast.finalize()?;
        Ok(ast)
    }

    fn next_node(&mut self) -> Result<Node, ParserError> {
        if self.is_at_end() {
            return Err(ParserError::Ast(
                AstError::StreamError("AtEnd".to_string()),
                None,
            ));
        }

        let token = self.peek()?;
        let node = match token.token_type() {
            TokenType::DjangoBlock(content) => {
                self.consume()?;
                self.parse_django_block(content)
            }
            TokenType::DjangoVariable(content) => {
                self.consume()?;
                self.parse_django_variable(content)
            }
            TokenType::Comment(content, start, end) => {
                self.consume()?;
                self.parse_comment(content, start, end.as_deref())
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
            | TokenType::StyleTagClose(_) => self.parse_text(),
            TokenType::Eof => Err(ParserError::Ast(
                AstError::StreamError("AtEnd".to_string()),
                None,
            )),
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
            "{#" => Ok(Node::Comment(content.to_string())),
            _ => Ok(Node::Text(format!(
                "{}{}{}",
                start,
                content,
                end.unwrap_or("")
            ))),
        }
    }

    fn parse_django_block(&mut self, s: &str) -> Result<Node, ParserError> {
        let bits: Vec<String> = s.split_whitespace().map(String::from).collect();
        let tag_name = bits.first().ok_or(AstError::EmptyTag)?.clone();

        let specs = TagSpec::load_builtin_specs().unwrap_or_default();

        // Check if this is a closing or branch tag
        for (_, spec) in specs.iter() {
            if Some(&tag_name) == spec.closing.as_ref()
                || spec
                    .branches
                    .as_ref()
                    .map(|ints| ints.iter().any(|i| i.name == tag_name))
                    .unwrap_or(false)
            {
                return Err(ParserError::ErrorSignal(Signal::SpecialTag(tag_name)));
            }
        }

        let tag_spec = specs.get(tag_name.as_str()).cloned();
        let mut children = Vec::new();
        let mut current_branch: Option<(String, Vec<String>, Vec<Node>)> = None;
        let mut found_closing_tag = false;

        while !self.is_at_end() {
            match self.next_node() {
                Ok(node) => {
                    if let Some((_, _, branch_children)) = &mut current_branch {
                        branch_children.push(node);
                    } else {
                        children.push(node);
                    }
                }
                Err(ParserError::ErrorSignal(Signal::SpecialTag(tag))) => {
                    if let Some(spec) = &tag_spec {
                        // Check if closing tag
                        if spec.closing.as_deref() == Some(&tag) {
                            // If we have a current branch, add it to children
                            if let Some((name, bits, branch_children)) = current_branch {
                                children.push(Node::Block {
                                    block_type: BlockType::Branch,
                                    name,
                                    bits,
                                    children: Some(branch_children),
                                });
                            }
                            children.push(Node::Block {
                                block_type: BlockType::Closing,
                                name: tag,
                                bits: vec![],
                                children: None,
                            });
                            found_closing_tag = true;
                            break;
                        }
                        // Check if intermediate tag
                        if let Some(branches) = &spec.branches {
                            if let Some(branch) = branches.iter().find(|i| i.name == tag) {
                                // If we have a current branch, add it to children
                                if let Some((name, bits, branch_children)) = current_branch {
                                    children.push(Node::Block {
                                        block_type: BlockType::Branch,
                                        name,
                                        bits,
                                        children: Some(branch_children),
                                    });
                                }
                                // Create new branch node
                                let branch_bits = if branch.args {
                                    match &self.tokens[self.current - 1].token_type() {
                                        TokenType::DjangoBlock(content) => content
                                            .split_whitespace()
                                            .skip(1) // Skip the tag name
                                            .map(|s| s.to_string())
                                            .collect(),
                                        _ => vec![tag.clone()],
                                    }
                                } else {
                                    vec![]
                                };
                                current_branch = Some((tag, branch_bits, Vec::new()));
                                continue;
                            }
                        }
                    }
                    // If we get here, it's an unexpected tag
                    let node = Node::Block {
                        block_type: BlockType::Standard,
                        name: tag_name.clone(),
                        bits: bits.clone(),
                        children: Some(children.clone()),
                    };
                    return Err(ParserError::Ast(AstError::UnexpectedTag(tag), Some(node)));
                }
                Err(ParserError::Ast(AstError::StreamError(kind), _)) if kind == "AtEnd" => {
                    break;
                }
                Err(e) => return Err(e),
            }
        }

        let node = Node::Block {
            block_type: BlockType::Standard,
            name: tag_name.clone(),
            bits,
            children: Some(children),
        };

        if !found_closing_tag {
            return Err(ParserError::Ast(
                AstError::UnclosedTag(tag_name),
                Some(node),
            ));
        }

        Ok(node)
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

        Ok(Node::Variable { bits, filters })
    }

    fn parse_text(&mut self) -> Result<Node, ParserError> {
        let mut text = String::new();
        while let Ok(token) = self.peek() {
            match token.token_type() {
                TokenType::DjangoBlock(_)
                | TokenType::DjangoVariable(_)
                | TokenType::Comment(_, _, _) => break,
                TokenType::Text(s) => {
                    self.consume()?;
                    text.push_str(s);
                }
                TokenType::HtmlTagOpen(s)
                | TokenType::HtmlTagClose(s)
                | TokenType::HtmlTagVoid(s)
                | TokenType::ScriptTagOpen(s)
                | TokenType::ScriptTagClose(s)
                | TokenType::StyleTagOpen(s)
                | TokenType::StyleTagClose(s) => {
                    self.consume()?;
                    text.push_str(s);
                }
                TokenType::Whitespace(len) => {
                    self.consume()?;
                    text.push_str(&" ".repeat(*len));
                }
                TokenType::Newline => {
                    self.consume()?;
                    text.push('\n');
                }
                TokenType::Eof => break,
            }
        }
        Ok(Node::Text(text))
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
        let sync_types = [
            TokenType::DjangoBlock(String::new()),
            TokenType::DjangoVariable(String::new()),
            TokenType::Comment(String::new(), String::from("{#"), Some(String::from("#}"))),
            TokenType::Eof,
        ];
        while !self.is_at_end() {
            let current = self.peek()?;
            for sync_type in &sync_types {
                if current.token_type() == sync_type {
                    return Ok(());
                }
            }
            self.consume()?;
        }
        Err(ParserError::Ast(
            AstError::StreamError("AtEnd".into()),
            None,
        ))
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

#[derive(Error, Debug)]
pub enum ParserError {
    #[error("ast error: {0}")]
    Ast(AstError, Option<Node>),
    #[error("internal signal: {0:?}")]
    ErrorSignal(Signal),
}

impl From<AstError> for ParserError {
    fn from(err: AstError) -> Self {
        ParserError::Ast(err, None)
    }
}

impl ParserError {
    pub fn stream_error(kind: impl Into<String>) -> Self {
        Self::Ast(AstError::StreamError(kind.into()), None)
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
        fn test_parse_unclosed_html_tag() {
            let source = "<div>";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let ast = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
            assert_eq!(ast.errors().len(), 0);
        }

        #[test]
        fn test_parse_unclosed_django_if() {
            let source = "{% if user.is_authenticated %}Welcome";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let ast = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
            assert_eq!(ast.errors().len(), 1);
            assert!(matches!(&ast.errors()[0], AstError::UnclosedTag(tag) if tag == "if"));
        }

        #[test]
        fn test_parse_unclosed_django_for() {
            let source = "{% for item in items %}{{ item.name }}";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let ast = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
            assert_eq!(ast.errors().len(), 1);
            assert!(matches!(&ast.errors()[0], AstError::UnclosedTag(tag) if tag == "for"));
        }

        #[test]
        fn test_parse_unclosed_script() {
            let source = "<script>console.log('test');";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let ast = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
            assert_eq!(ast.errors().len(), 0);
        }

        #[test]
        fn test_parse_unclosed_style() {
            let source = "<style>body { color: blue; ";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let ast = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
            assert_eq!(ast.errors().len(), 0);
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
            let ast = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
            assert_eq!(ast.errors().len(), 1);
            assert!(matches!(&ast.errors()[0], AstError::UnclosedTag(tag) if tag == "if"));
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
            let ast = parser.parse().unwrap();
            insta::assert_yaml_snapshot!(ast);
        }
    }
}

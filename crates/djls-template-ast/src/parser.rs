use crate::ast::{Ast, AstError, BlockType, DjangoFilter, LineOffsets, Node, Span};
use crate::tagspecs::{TagSpec, TagType};
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
        let mut line_offsets = LineOffsets::new();  // Already contains [0]

        // First pass: collect line offsets
        for token in self.tokens.tokens() {
            match token.token_type() {
                TokenType::Newline => {
                    if let Some(start) = token.start() {
                        eprintln!("Parser: Found newline at position {}", start);
                        // Add line offset at the start of the next line
                        line_offsets.add_line(*start as u32 + 1);
                        eprintln!("Parser: Added line offset {} at position {}", line_offsets.0.len(), start + 1);
                    }
                }
                TokenType::Whitespace(count) => {
                    if let Some(start) = token.start() {
                        eprintln!("Parser: Found whitespace of length {} at position {}", count, start);
                    }
                }
                _ => {}
            }
        }

        // Reset current position
        self.current = 0;

        // Second pass: parse nodes
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

        ast.set_line_offsets(line_offsets);
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
        let start_token = self.peek_previous()?;
        let start_pos = start_token.start().unwrap_or(0) as u32;
        let total_length = content.len() + start.len() + end.map_or(0, |e| e.len());
        let span = Span::new(start_pos, total_length as u16);
        Ok(Node::Comment {
            content: content.to_string(),
            span,
        })
    }

    fn parse_django_block(&mut self, s: &str) -> Result<Node, ParserError> {
        let token = self.peek_previous()?;
        let start = *token.start().unwrap();
        let length = token.length().unwrap();
        let span = Span::new(start as u32, length as u32);

        // Parse the tag content
        let bits: Vec<String> = s.split_whitespace().map(String::from).collect();
        if bits.is_empty() {
            return Err(ParserError::Ast(
                AstError::InvalidTag("Empty tag".to_string()),
                None,
            ));
        }

        let tag_name = bits[0].clone();
        
        // Check for assignments
        let mut assignments = None;
        let mut assignment = None;
        if let Some(assign_idx) = bits.iter().position(|b| b.contains('=')) {
            let mut assign_vec = Vec::new();
            for bit in &bits[assign_idx..] {
                if let Some((target, value)) = bit.split_once('=') {
                    assign_vec.push(Assignment {
                        target: target.trim().to_string(),
                        value: value.trim_matches('"').to_string(),
                    });
                }
            }
            if !assign_vec.is_empty() {
                assignments = Some(assign_vec);
            }
        }

        let tag = Tag {
            name: tag_name.clone(),
            bits,
            span,
            tag_span: span,  // For now, tag_span is same as full span
            assignment,
        };

        // Handle different tag types based on tagspecs
        let spec = match TagSpec::load_builtin_specs()?.get(&tag_name) {
            Some(spec) => spec.clone(),
            None => return Ok(Node::Block(Block::Tag { tag })),
        };

        match spec.tag_type {
            TagType::Block => {
                let mut nodes = Vec::new();
                
                // Parse child nodes until we find the closing tag
                while !self.is_at_end() {
                    match self.next_node() {
                        Ok(node) => nodes.push(node),
                        Err(ParserError::ErrorSignal(Signal::ClosingTagFound(closing_name))) => {
                            let closing_tag = Tag {
                                name: closing_name,
                                bits: vec![],
                                span,
                                tag_span: span,  // For now, tag_span is same as full span
                                assignment: None,
                            };
                            return Ok(Node::Block(Block::Block {
                                tag,
                                nodes,
                                closing: Some(Box::new(Block::Closing { tag: closing_tag })),
                                assignments,
                            }));
                        }
                        Err(e) => return Err(e),
                    }
                }

                Err(ParserError::Ast(
                    AstError::UnclosedBlock(tag_name),
                    Some(Node::Block(Block::Block {
                        tag,
                        nodes,
                        closing: None,
                        assignments,
                    })),
                ))
            }
            TagType::Tag => Ok(Node::Block(Block::Tag { tag })),
            TagType::Variable => Ok(Node::Block(Block::Variable { tag })),
            TagType::Assignment => {
                if let Some(target) = assignment {
                    let mut nodes = Vec::new();
                    while !self.is_at_end() {
                        match self.next_node() {
                            Ok(node) => nodes.push(node),
                            Err(ParserError::ErrorSignal(Signal::ClosingTagFound(_))) => {
                                return Ok(Node::Block(Block::Block {
                                    tag,
                                    nodes,
                                    closing: None,
                                    assignments: Some(vec![Assignment {
                                        target,
                                        value: bits[1..].join(" "),
                                    }]),
                                }));
                            }
                            Err(e) => return Err(e),
                        }
                    }
                    Ok(Node::Block(Block::Block {
                        tag,
                        nodes,
                        closing: None,
                        assignments: Some(vec![Assignment {
                            target,
                            value: bits[1..].join(" "),
                        }]),
                    }))
                } else {
                    Ok(Node::Block(Block::Tag { tag }))
                }
            }
        }
    }

    fn parse_django_variable(&mut self, s: &str) -> Result<Node, ParserError> {
        let token = self.peek_previous()?;
        let start = *token.start().unwrap();
        let length = token.length().unwrap();
        let span = Span::new(start as u32, length as u32);

        // Split into variable and filters
        let parts: Vec<&str> = s.split('|').collect();
        let bits: Vec<String> = parts[0]
            .trim()
            .split('.')
            .map(|s| s.trim().to_string())
            .collect();

        let mut filters = Vec::new();
        if parts.len() > 1 {
            for filter_part in &parts[1..] {
                let filter_parts: Vec<&str> = filter_part.trim().split(':').collect();
                let name = filter_parts[0].trim().to_string();
                let arguments = if filter_parts.len() > 1 {
                    filter_parts[1]
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .collect()
                } else {
                    Vec::new()
                };

                filters.push(DjangoFilter::new(name, arguments, span));
            }
        }

        Ok(Node::Variable {
            bits,
            filters,
            span,
        })
    }

    fn parse_text(&mut self) -> Result<Node, ParserError> {
        let mut start_pos = self.peek()?.start().unwrap_or(0) as u32;
        let mut text = String::new();

        // Skip any leading newlines and whitespace
        while let Ok(token) = self.peek() {
            match token.token_type() {
                TokenType::Newline => {
                    self.consume()?;
                    text.push('\n');
                    if let Ok(next) = self.peek() {
                        start_pos = next.start().unwrap_or(0) as u32;
                    }
                }
                TokenType::Whitespace(len) => {
                    self.consume()?;
                    text.push_str(&" ".repeat(*len));
                    if let Ok(next) = self.peek() {
                        start_pos = next.start().unwrap_or(0) as u32;
                    }
                }
                _ => break,
            }
        }

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

        let length = text.len() as u16;
        let span = Span::new(start_pos, length);
        Ok(Node::Text {
            content: text,
            span,
        })
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
    use crate::tokens::Token;

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
            let source =
                "{% if x > 0 %}Positive{% elif x < 0 %}Negative{% else %}Zero{% endif %}";
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

    mod line_tracking {
        use super::*;

        #[test]
        fn test_parser_tracks_line_offsets() {
            let source = "line1\nline2";
            let tokens = Lexer::new(source).tokenize().unwrap();
            let mut parser = Parser::new(tokens);
            let ast = parser.parse().unwrap();

            let offsets = ast.line_offsets();
            eprintln!("{:?}", offsets);
            assert_eq!(offsets.position_to_line_col(0), (0, 0)); // Start of line 1
            assert_eq!(offsets.position_to_line_col(6), (1, 0)); // Start of line 2
        }
    }
}

use serde::Serialize;
use thiserror::Error;

use crate::ast::FilterName;
use crate::ast::Node;
use crate::ast::NodeList;
use crate::ast::NodeListError;
use crate::ast::Span;
use crate::ast::TagName;
use crate::ast::VariableName;
use crate::db::Db as TemplateDb;
use crate::tokens::Token;
use crate::tokens::TokenStream;

pub struct Parser<'db> {
    db: &'db dyn TemplateDb,
    tokens: TokenStream<'db>,
    current: usize,
    errors: Vec<ParserError>,
}

impl<'db> Parser<'db> {
    #[must_use]
    pub fn new(db: &'db dyn TemplateDb, tokens: TokenStream<'db>) -> Self {
        Self {
            db,
            tokens,
            current: 0,
            errors: Vec::new(),
        }
    }

    pub fn parse(&mut self) -> Result<(NodeList<'db>, Vec<ParserError>), ParserError> {
        let mut nodelist = Vec::new();
        let mut line_offsets = crate::ast::LineOffsets::default();

        let tokens = self.tokens.stream(self.db);
        for token in tokens {
            if matches!(token, Token::Newline { .. }) {
                let offset = token.offset();
                if let Some(offset) = offset {
                    line_offsets.add_line(offset + 1);
                }
            }
        }

        while !self.is_at_end() {
            match self.next_node() {
                Ok(node) => {
                    nodelist.push(node);
                }
                Err(err) => {
                    if !self.is_at_end() {
                        self.errors.push(err);
                        self.synchronize()?;
                    }
                }
            }
        }

        let ast = NodeList::new(self.db, nodelist, line_offsets);

        Ok((ast, std::mem::take(&mut self.errors)))
    }

    fn next_node(&mut self) -> Result<Node<'db>, ParserError> {
        let token = self.consume()?;

        match token {
            Token::Comment { .. } => self.parse_comment(),
            Token::Eof { .. } => Err(ParserError::stream_error(StreamError::AtEnd)),
            Token::Block { .. } => self.parse_django_block(),
            Token::Variable { .. } => self.parse_django_variable(),
            Token::Error { .. } => self.parse_error(),
            Token::Newline { .. } | Token::Text { .. } | Token::Whitespace { .. } => {
                self.parse_text()
            }
        }
    }

    fn parse_comment(&mut self) -> Result<Node<'db>, ParserError> {
        let token = self.peek_previous()?;

        Ok(Node::Comment {
            content: token.content(self.db),
            span: Span::from_token(&token, self.db),
        })
    }

    fn parse_error(&mut self) -> Result<Node<'db>, ParserError> {
        let token = self.peek_previous()?;

        if let Token::Error {
            content, offset, ..
        } = token
        {
            let error_text = content.text(self.db).clone();

            Err(ParserError::MalformedConstruct {
                position: offset,
                content: error_text,
            })
        } else {
            Err(ParserError::InvalidSyntax {
                context: "Expected Error token".to_string(),
            })
        }
    }

    pub fn parse_django_block(&mut self) -> Result<Node<'db>, ParserError> {
        let token = self.peek_previous()?;

        let content = token.content(self.db);
        let args: Vec<String> = content.split_whitespace().map(String::from).collect();
        let name_str = args.first().ok_or(ParserError::EmptyTag)?.clone();
        let name = TagName::new(self.db, name_str); // Intern the tag name
        let bits = args.into_iter().skip(1).collect();
        let span = Span::from_token(&token, self.db);

        Ok(Node::Tag { name, bits, span })
    }

    fn parse_django_variable(&mut self) -> Result<Node<'db>, ParserError> {
        let token = self.peek_previous()?;

        let content = token.content(self.db);
        let bits: Vec<&str> = content.split('|').collect();
        let var_str = bits
            .first()
            .ok_or(ParserError::EmptyTag)?
            .trim()
            .to_string();
        let var = VariableName::new(self.db, var_str); // Intern the variable name
        let filters = bits
            .into_iter()
            .skip(1)
            .map(|s| FilterName::new(self.db, s.trim().to_string())) // Intern filter names
            .collect();
        let span = Span::from_token(&token, self.db);

        Ok(Node::Variable { var, filters, span })
    }

    fn parse_text(&mut self) -> Result<Node<'db>, ParserError> {
        let first_token = self.peek_previous()?;

        // Skip standalone newlines
        if matches!(first_token, Token::Newline { .. }) {
            return self.next_node();
        }

        let offset = first_token.offset().unwrap_or(0);
        let mut end_position = offset + first_token.length(self.db);

        while let Ok(token) = self.peek() {
            match token {
                Token::Block { .. }
                | Token::Variable { .. }
                | Token::Comment { .. }
                | Token::Error { .. }
                | Token::Eof { .. } => break, // Stop at Django constructs
                Token::Text { .. } | Token::Whitespace { .. } | Token::Newline { .. } => {
                    // Update end position
                    let token_offset = token.offset().unwrap_or(end_position);
                    let token_length = token.length(self.db);
                    end_position = token_offset + token_length;
                    self.consume()?;
                }
            }
        }

        let length = end_position - offset;
        let span = Span::new(offset, length);

        Ok(Node::Text { span })
    }

    fn peek(&self) -> Result<Token<'db>, ParserError> {
        self.peek_at(0)
    }

    #[allow(dead_code)]
    fn peek_next(&self) -> Result<Token<'db>, ParserError> {
        self.peek_at(1)
    }

    fn peek_previous(&self) -> Result<Token<'db>, ParserError> {
        self.peek_at(-1)
    }

    #[allow(clippy::cast_sign_loss)]
    fn peek_at(&self, offset: isize) -> Result<Token<'db>, ParserError> {
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

    fn item_at(&self, index: usize) -> Result<Token<'db>, ParserError> {
        let tokens = self.tokens.stream(self.db);
        if let Some(token) = tokens.get(index) {
            Ok(token.clone())
        } else {
            let error = if tokens.is_empty() {
                ParserError::stream_error(StreamError::Empty)
            } else if index < self.current {
                ParserError::stream_error(StreamError::AtBeginning)
            } else if index >= tokens.len() {
                ParserError::stream_error(StreamError::AtEnd)
            } else {
                ParserError::stream_error(StreamError::InvalidAccess)
            };
            Err(error)
        }
    }

    fn is_at_end(&self) -> bool {
        let tokens = self.tokens.stream(self.db);
        self.current + 1 >= tokens.len()
    }

    fn consume(&mut self) -> Result<Token<'db>, ParserError> {
        if self.is_at_end() {
            return Err(ParserError::stream_error(StreamError::AtEnd));
        }
        self.current += 1;
        self.peek_previous()
    }

    #[allow(dead_code)]
    fn backtrack(&mut self, steps: usize) -> Result<Token<'db>, ParserError> {
        if self.current < steps {
            return Err(ParserError::stream_error(StreamError::AtBeginning));
        }
        self.current -= steps;
        self.peek_next()
    }

    fn synchronize(&mut self) -> Result<(), ParserError> {
        while !self.is_at_end() {
            let current = self.peek()?;
            match current {
                Token::Block { .. }
                | Token::Variable { .. }
                | Token::Comment { .. }
                | Token::Eof { .. } => {
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

    #[error("AST error: {0}")]
    NodeList(#[from] NodeListError),
}

// Keep ParserError as alias for compatibility
pub type ParserError = ParseError;

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

    // Test database that implements the required traits
    #[salsa::db]
    #[derive(Clone)]
    struct TestDatabase {
        storage: salsa::Storage<Self>,
    }

    impl TestDatabase {
        fn new() -> Self {
            Self {
                storage: salsa::Storage::default(),
            }
        }
    }

    #[salsa::db]
    impl salsa::Database for TestDatabase {}

    #[salsa::db]
    impl djls_workspace::Db for TestDatabase {
        fn fs(&self) -> std::sync::Arc<dyn djls_workspace::FileSystem> {
            use djls_workspace::InMemoryFileSystem;
            static FS: std::sync::OnceLock<std::sync::Arc<InMemoryFileSystem>> =
                std::sync::OnceLock::new();
            FS.get_or_init(|| std::sync::Arc::new(InMemoryFileSystem::default()))
                .clone()
        }

        fn read_file_content(&self, path: &std::path::Path) -> Result<String, std::io::Error> {
            std::fs::read_to_string(path)
        }
    }

    #[salsa::db]
    impl crate::db::Db for TestDatabase {
        fn tag_specs(&self) -> std::sync::Arc<crate::templatetags::TagSpecs> {
            std::sync::Arc::new(
                crate::templatetags::TagSpecs::load_builtin_specs()
                    .unwrap_or_else(|_| crate::templatetags::TagSpecs::default()),
            )
        }
    }

    #[salsa::input]
    struct TestTemplate {
        #[returns(ref)]
        source: String,
    }

    #[salsa::tracked]
    fn parse_test_template(db: &dyn TemplateDb, template: TestTemplate) -> NodeList<'_> {
        let source = template.source(db);
        let tokens = Lexer::new(db, source).tokenize();
        let token_stream = TokenStream::new(db, tokens);
        let mut parser = Parser::new(db, token_stream);
        let (ast, _) = parser.parse().unwrap();
        ast
    }

    #[derive(Debug, Clone, PartialEq, Serialize)]
    struct TestAst {
        nodelist: Vec<TestNode>,
        line_offsets: Vec<u32>,
    }

    #[derive(Debug, Clone, PartialEq, Serialize)]
    #[serde(tag = "type")]
    enum TestNode {
        Tag {
            name: String,
            bits: Vec<String>,
            span: (u32, u32),
        },
        Comment {
            content: String,
            span: (u32, u32),
        },
        Text {
            span: (u32, u32),
        },
        Variable {
            var: String,
            filters: Vec<String>,
            span: (u32, u32),
        },
    }

    impl TestNode {
        fn from_node(node: &Node<'_>, db: &dyn crate::db::Db) -> Self {
            match node {
                Node::Tag { name, bits, span } => TestNode::Tag {
                    name: name.text(db).to_string(),
                    bits: bits.clone(),
                    span: (span.start, span.length),
                },
                Node::Comment { content, span } => TestNode::Comment {
                    content: content.clone(),
                    span: (span.start, span.length),
                },
                Node::Text { span } => TestNode::Text {
                    span: (span.start, span.length),
                },
                Node::Variable { var, filters, span } => TestNode::Variable {
                    var: var.text(db).to_string(),
                    filters: filters.iter().map(|f| f.text(db).to_string()).collect(),
                    span: (span.start, span.length),
                },
            }
        }
    }

    fn convert_ast_for_testing(ast: NodeList<'_>, db: &dyn crate::db::Db) -> TestAst {
        TestAst {
            nodelist: convert_nodelist_for_testing(ast.nodelist(db), db),
            line_offsets: ast.line_offsets(db).0.clone(),
        }
    }

    fn convert_nodelist_for_testing(nodes: &[Node<'_>], db: &dyn crate::db::Db) -> Vec<TestNode> {
        nodes.iter().map(|n| TestNode::from_node(n, db)).collect()
    }

    mod html {
        use super::*;

        #[test]
        fn test_parse_html_doctype() {
            let db = TestDatabase::new();
            let source = "<!DOCTYPE html>".to_string();
            let template = TestTemplate::new(&db, source);
            let ast = parse_test_template(&db, template);
            let test_ast = convert_ast_for_testing(ast, &db);
            insta::assert_yaml_snapshot!(test_ast);
        }

        #[test]
        fn test_parse_html_tag() {
            let db = TestDatabase::new();
            let source = "<div class=\"container\">Hello</div>".to_string();
            let template = TestTemplate::new(&db, source);
            let ast = parse_test_template(&db, template);
            let test_ast = convert_ast_for_testing(ast, &db);
            insta::assert_yaml_snapshot!(test_ast);
        }

        #[test]
        fn test_parse_html_void() {
            let db = TestDatabase::new();
            let source = "<input type=\"text\" />".to_string();
            let template = TestTemplate::new(&db, source);
            let ast = parse_test_template(&db, template);
            let test_ast = convert_ast_for_testing(ast, &db);
            insta::assert_yaml_snapshot!(test_ast);
        }
    }

    mod django {
        use super::*;

        #[test]
        fn test_parse_django_variable() {
            let db = TestDatabase::new();
            let source = "{{ user.name }}".to_string();
            let template = TestTemplate::new(&db, source);
            let ast = parse_test_template(&db, template);
            let test_ast = convert_ast_for_testing(ast, &db);
            insta::assert_yaml_snapshot!(test_ast);
        }

        #[test]
        fn test_parse_django_variable_with_filter() {
            let db = TestDatabase::new();
            let source = "{{ user.name|title }}".to_string();
            let template = TestTemplate::new(&db, source);
            let ast = parse_test_template(&db, template);
            let test_ast = convert_ast_for_testing(ast, &db);
            insta::assert_yaml_snapshot!(test_ast);
        }

        #[test]
        fn test_parse_filter_chains() {
            let db = TestDatabase::new();
            let source = "{{ value|default:'nothing'|title|upper }}".to_string();
            let template = TestTemplate::new(&db, source);
            let ast = parse_test_template(&db, template);
            let test_ast = convert_ast_for_testing(ast, &db);
            insta::assert_yaml_snapshot!(test_ast);
        }

        #[test]
        fn test_parse_django_if_block() {
            let db = TestDatabase::new();
            let source = "{% if user.is_authenticated %}Welcome{% endif %}".to_string();
            let template = TestTemplate::new(&db, source);
            let ast = parse_test_template(&db, template);
            let test_ast = convert_ast_for_testing(ast, &db);
            insta::assert_yaml_snapshot!(test_ast);
        }

        #[test]
        fn test_parse_django_for_block() {
            let db = TestDatabase::new();
            let source =
                "{% for item in items %}{{ item }}{% empty %}No items{% endfor %}".to_string();
            let template = TestTemplate::new(&db, source);
            let ast = parse_test_template(&db, template);
            let test_ast = convert_ast_for_testing(ast, &db);
            insta::assert_yaml_snapshot!(test_ast);
        }

        #[test]
        fn test_parse_complex_if_elif() {
            let db = TestDatabase::new();
            let source = "{% if x > 0 %}Positive{% elif x < 0 %}Negative{% else %}Zero{% endif %}"
                .to_string();
            let template = TestTemplate::new(&db, source);
            let ast = parse_test_template(&db, template);
            let test_ast = convert_ast_for_testing(ast, &db);
            insta::assert_yaml_snapshot!(test_ast);
        }

        #[test]
        fn test_parse_django_tag_assignment() {
            let db = TestDatabase::new();
            let source = "{% url 'view-name' as view %}".to_string();
            let template = TestTemplate::new(&db, source);
            let ast = parse_test_template(&db, template);
            let test_ast = convert_ast_for_testing(ast, &db);
            insta::assert_yaml_snapshot!(test_ast);
        }

        #[test]
        fn test_parse_nested_for_if() {
            let db = TestDatabase::new();
            let source =
                "{% for item in items %}{% if item.active %}{{ item.name }}{% endif %}{% endfor %}"
                    .to_string();
            let template = TestTemplate::new(&db, source);
            let ast = parse_test_template(&db, template);
            let test_ast = convert_ast_for_testing(ast, &db);
            insta::assert_yaml_snapshot!(test_ast);
        }

        #[test]
        fn test_parse_mixed_content() {
            let db = TestDatabase::new();
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
{% endif %}!"
                .to_string();
            let template = TestTemplate::new(&db, source);
            let ast = parse_test_template(&db, template);
            let test_ast = convert_ast_for_testing(ast, &db);
            insta::assert_yaml_snapshot!(test_ast);
        }
    }

    mod script {
        use super::*;

        #[test]
        fn test_parse_script() {
            let db = TestDatabase::new();
            let source = r#"<script type="text/javascript">
    // Single line comment
    const x = 1;
    /* Multi-line
        comment */
    console.log(x);
</script>"#
                .to_string();
            let template = TestTemplate::new(&db, source);
            let ast = parse_test_template(&db, template);
            let test_ast = convert_ast_for_testing(ast, &db);
            insta::assert_yaml_snapshot!(test_ast);
        }
    }

    mod style {
        use super::*;

        #[test]
        fn test_parse_style() {
            let db = TestDatabase::new();
            let source = r#"<style type="text/css">
    /* Header styles */
    .header {
        color: blue;
    }
</style>"#
                .to_string();
            let template = TestTemplate::new(&db, source);
            let ast = parse_test_template(&db, template);
            let test_ast = convert_ast_for_testing(ast, &db);
            insta::assert_yaml_snapshot!(test_ast);
        }
    }

    mod comments {
        use super::*;

        #[test]
        fn test_parse_comments() {
            let db = TestDatabase::new();
            let source = "<!-- HTML comment -->{# Django comment #}".to_string();
            let template = TestTemplate::new(&db, source);
            let ast = parse_test_template(&db, template);
            let test_ast = convert_ast_for_testing(ast, &db);
            insta::assert_yaml_snapshot!(test_ast);
        }
    }

    mod whitespace {
        use super::*;

        #[test]
        fn test_parse_with_leading_whitespace() {
            let db = TestDatabase::new();
            let source = "     hello".to_string();
            let template = TestTemplate::new(&db, source);
            let ast = parse_test_template(&db, template);
            let test_ast = convert_ast_for_testing(ast, &db);
            insta::assert_yaml_snapshot!(test_ast);
        }

        #[test]
        fn test_parse_with_leading_whitespace_newline() {
            let db = TestDatabase::new();
            let source = "\n     hello".to_string();
            let template = TestTemplate::new(&db, source);
            let ast = parse_test_template(&db, template);
            let test_ast = convert_ast_for_testing(ast, &db);
            insta::assert_yaml_snapshot!(test_ast);
        }

        #[test]
        fn test_parse_with_trailing_whitespace() {
            let db = TestDatabase::new();
            let source = "hello     ".to_string();
            let template = TestTemplate::new(&db, source);
            let ast = parse_test_template(&db, template);
            let test_ast = convert_ast_for_testing(ast, &db);
            insta::assert_yaml_snapshot!(test_ast);
        }

        #[test]
        fn test_parse_with_trailing_whitespace_newline() {
            let db = TestDatabase::new();
            let source = "hello     \n".to_string();
            let template = TestTemplate::new(&db, source);
            let ast = parse_test_template(&db, template);
            let test_ast = convert_ast_for_testing(ast, &db);
            insta::assert_yaml_snapshot!(test_ast);
        }
    }

    mod errors {
        use super::*;

        #[test]
        fn test_parse_unclosed_html_tag() {
            let db = TestDatabase::new();
            let source = "<div>".to_string();
            let template = TestTemplate::new(&db, source);
            let ast = parse_test_template(&db, template);
            let test_ast = convert_ast_for_testing(ast, &db);
            insta::assert_yaml_snapshot!(test_ast);
        }

        #[test]
        fn test_parse_unclosed_django_if() {
            let db = TestDatabase::new();
            let source = "{% if user.is_authenticated %}Welcome".to_string();
            let template = TestTemplate::new(&db, source);
            let ast = parse_test_template(&db, template);
            let test_ast = convert_ast_for_testing(ast, &db);
            insta::assert_yaml_snapshot!(test_ast);
        }

        #[test]
        fn test_parse_unclosed_django_for() {
            let db = TestDatabase::new();
            let source = "{% for item in items %}{{ item.name }}".to_string();
            let template = TestTemplate::new(&db, source);
            let ast = parse_test_template(&db, template);
            let test_ast = convert_ast_for_testing(ast, &db);
            insta::assert_yaml_snapshot!(test_ast);
        }

        #[test]
        fn test_parse_unclosed_script() {
            let db = TestDatabase::new();
            let source = "<script>console.log('test');".to_string();
            let template = TestTemplate::new(&db, source);
            let ast = parse_test_template(&db, template);
            let test_ast = convert_ast_for_testing(ast, &db);
            insta::assert_yaml_snapshot!(test_ast);
        }

        #[test]
        fn test_parse_unclosed_style() {
            let db = TestDatabase::new();
            let source = "<style>body { color: blue; ".to_string();
            let template = TestTemplate::new(&db, source);
            let ast = parse_test_template(&db, template);
            let test_ast = convert_ast_for_testing(ast, &db);
            insta::assert_yaml_snapshot!(test_ast);
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
        //     let (ast, errors) = parser.parse().unwrap();
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
            let db = TestDatabase::new();
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
            let template = TestTemplate::new(&db, source);
            let ast = parse_test_template(&db, template);
            let test_ast = convert_ast_for_testing(ast, &db);
            insta::assert_yaml_snapshot!(test_ast);
        }
    }

    mod line_tracking {
        use super::*;

        #[test]
        fn test_parser_tracks_line_offsets() {
            let db = TestDatabase::new();
            let source = "line1\nline2".to_string();
            let template = TestTemplate::new(&db, source);
            let ast = parse_test_template(&db, template);

            let offsets = ast.line_offsets(&db);
            assert_eq!(offsets.position_to_line_col(0), (1, 0)); // Start of line 1
            assert_eq!(offsets.position_to_line_col(6), (2, 0)); // Start of line 2
        }
    }
}

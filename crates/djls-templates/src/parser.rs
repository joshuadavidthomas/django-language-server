use thiserror::Error;

use crate::ast::CommentNode;
use crate::ast::FilterName;
use crate::ast::Node;
use crate::ast::NodeList;
use crate::ast::NodeListError;
use crate::ast::Span;
use crate::ast::TagName;
use crate::ast::TagNode;
use crate::ast::TextNode;
use crate::ast::VariableName;
use crate::ast::VariableNode;
use crate::db::Db as TemplateDb;

use crate::lexer::LexerError;
use crate::syntax_tree::ParsedArg;
use crate::syntax_tree::ParsedArgs;
use crate::syntax_tree::SyntaxNode;
use crate::syntax_tree::SyntaxNodeId;
use crate::syntax_tree::SyntaxTree;
use crate::syntax_tree::TagMeta;
use crate::syntax_tree::TagShape;
use crate::templatetags::TagSpecs;
use crate::templatetags::TagType;
use crate::tokens::Token;
use crate::tokens::TokenStream;
use crate::tokens::TokenType;

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

        // Build line offsets from tokens
        let tokens = self.tokens.stream(self.db);
        for token in tokens {
            if let TokenType::Newline = token.token_type() {
                if let Some(start) = token.start() {
                    // Add offset for next line
                    line_offsets.add_line(start + 1);
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

        // Create the tracked NodeList struct
        let ast = NodeList::new(self.db, nodelist, line_offsets);

        Ok((ast, std::mem::take(&mut self.errors)))
    }



    fn next_node(&mut self) -> Result<Node<'db>, ParserError> {
        let token = self.consume()?;

        match token.token_type() {
            TokenType::Comment(_, open, _) => self.parse_comment(open),
            TokenType::Eof => Err(ParserError::stream_error(StreamError::AtEnd)),
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

    fn parse_comment(&mut self, open: &str) -> Result<Node<'db>, ParserError> {
        // Only treat Django comments as Comment nodes
        if open != "{#" {
            return self.parse_text();
        }

        let token = self.peek_previous()?;

        Ok(Node::Comment(CommentNode {
            content: token.content(),
            span: Span::from_token(&token),
        }))
    }

    pub fn parse_django_block(&mut self) -> Result<Node<'db>, ParserError> {
        let token = self.peek_previous()?;

        let args: Vec<String> = token
            .content()
            .split_whitespace()
            .map(String::from)
            .collect();
        let name_str = args.first().ok_or(ParserError::EmptyTag)?.clone();
        let name = TagName::new(self.db, name_str); // Intern the tag name
        let bits = args.into_iter().skip(1).collect();
        let span = Span::from_token(&token);

        Ok(Node::Tag(TagNode { name, bits, span }))
    }

    fn parse_django_variable(&mut self) -> Result<Node<'db>, ParserError> {
        let token = self.peek_previous()?;

        let content = token.content();
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
        let span = Span::from_token(&token);

        Ok(Node::Variable(VariableNode { var, filters, span }))
    }

    fn parse_text(&mut self) -> Result<Node<'db>, ParserError> {
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
        let offset = u32::try_from(text.find(content.as_str()).unwrap_or(0))
            .expect("Offset should fit in u32");
        let length = u32::try_from(content.len()).expect("Content length should fit in u32");
        let span = Span::new(start + offset, length);

        Ok(Node::Text(TextNode { content, span }))
    }

    fn peek(&self) -> Result<Token, ParserError> {
        self.peek_at(0)
    }

    #[allow(dead_code)]
    fn peek_next(&self) -> Result<Token, ParserError> {
        self.peek_at(1)
    }

    fn peek_previous(&self) -> Result<Token, ParserError> {
        self.peek_at(-1)
    }

    #[allow(clippy::cast_sign_loss)]
    fn peek_at(&self, offset: isize) -> Result<Token, ParserError> {
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

    fn item_at(&self, index: usize) -> Result<Token, ParserError> {
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

    fn consume(&mut self) -> Result<Token, ParserError> {
        if self.is_at_end() {
            return Err(ParserError::stream_error(StreamError::AtEnd));
        }
        self.current += 1;
        self.peek_previous()
    }

    #[allow(dead_code)]
    fn backtrack(&mut self, steps: usize) -> Result<Token, ParserError> {
        if self.current < steps {
            return Err(ParserError::stream_error(StreamError::AtBeginning));
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

/// Build a `SyntaxTree` from an existing `NodeList`
pub fn build_syntax_tree<'db>(db: &'db dyn TemplateDb, nodelist: &'db NodeList<'db>) -> Result<SyntaxTree<'db>, ParserError> {
    let mut tree_builder = TreeBuilder::new(db, nodelist);
    tree_builder.build_tree()
}

#[derive(Debug)]
pub enum StreamError {
    AtBeginning,
    BeforeStart,
    AtEnd,
    Empty,
    InvalidAccess,
}

#[derive(Debug, Error)]
pub enum ParserError {
    #[error("Unexpected token: expected {expected:?}, found {found} at position {position}")]
    UnexpectedToken {
        expected: Vec<String>,
        found: String,
        position: usize,
    },
    #[error("Invalid syntax: {context}")]
    InvalidSyntax { context: String },
    #[error("Empty tag")]
    EmptyTag,
    #[error("Lexer error: {0}")]
    Lexer(#[from] LexerError),
    #[error("Stream error: {kind:?}")]
    StreamError { kind: StreamError },
    #[error("AST error: {0}")]
    NodeList(#[from] NodeListError),
}

impl ParserError {
    pub fn stream_error(kind: impl Into<StreamError>) -> Self {
        Self::StreamError { kind: kind.into() }
    }
}

/// Two-stage parser: `TreeBuilder` converts flat `NodeList` to hierarchical `SyntaxTree`
pub struct TreeBuilder<'db> {
    db: &'db dyn TemplateDb,
    nodelist: &'db NodeList<'db>,
    stack: Vec<StackFrame<'db>>,
}

struct StackFrame<'db> {
    opener_tag: crate::ast::TagNode<'db>,
    children: Vec<SyntaxNodeId<'db>>,
}

impl<'db> TreeBuilder<'db> {
    pub fn new(db: &'db dyn TemplateDb, nodelist: &'db NodeList<'db>) -> Self {
        Self {
            db,
            nodelist,
            stack: Vec::new(),
        }
    }

    pub fn build_tree(&mut self) -> Result<SyntaxTree<'db>, ParserError> {
        let mut root_children = Vec::new();
        let tag_specs = self.db.tag_specs();
        
        for node in self.nodelist.nodelist(self.db) {
            match node {
                Node::Tag(tag_node) => {
                    self.handle_tag_node(tag_node, &tag_specs, &mut root_children)?;
                }
                
                Node::Text(_) | Node::Variable(_) | Node::Comment(_) => {
                    let syntax_node = self.convert_node(node);
                    let node_id = SyntaxNodeId::new(self.db, syntax_node);
                    self.add_to_current_container(node_id, &mut root_children);
                }
            }
        }

        // Check for unclosed blocks
        if !self.stack.is_empty() {
            return Err(ParserError::InvalidSyntax { 
                context: "Unclosed blocks remaining".to_string() 
            });
        }
        
        let root = SyntaxNode::Root { children: root_children };
        let root_id = SyntaxNodeId::new(self.db, root);
        
        Ok(SyntaxTree::new(
            self.db, 
            root_id, 
            self.nodelist.line_offsets(self.db).clone()
        ))
    }

    fn handle_tag_node(
        &mut self,
        tag_node: &TagNode<'db>,
        tag_specs: &TagSpecs,
        root_children: &mut Vec<SyntaxNodeId<'db>>,
    ) -> Result<(), ParserError> {
        let name = tag_node.name.text(self.db);
        let tag_type = TagType::for_name(&name, tag_specs);
        

        
        match tag_type {
            TagType::Opener => self.handle_opener(tag_node, tag_specs, root_children),
            TagType::Intermediate => { self.handle_intermediate(tag_node); Ok(()) },
            TagType::Closer => self.handle_closer(tag_node, root_children),
            TagType::Standalone => self.handle_standalone(tag_node, tag_specs, root_children),
        }
    }

    fn handle_opener(
        &mut self,
        tag_node: &TagNode<'db>,
        tag_specs: &TagSpecs,
        root_children: &mut Vec<SyntaxNodeId<'db>>,
    ) -> Result<(), ParserError> {
        let name = tag_node.name.text(self.db);
        let spec = tag_specs.get(&name).cloned();
        let shape = spec.as_ref()
            .map_or(TagShape::Singleton, TagShape::from_spec);
        
        // If it's a block, push to stack and collect children
        if let TagShape::Block { .. } | TagShape::RawBlock { .. } = shape {
            self.stack.push(StackFrame {
                opener_tag: tag_node.clone(),
                children: Vec::new(),
            });
        } else {
            // Standalone tag that happens to be classified as opener (shouldn't happen normally)
            self.handle_standalone(tag_node, tag_specs, root_children)?;
        }
        
        Ok(())
    }

    fn handle_intermediate(
        &mut self,
        _tag_node: &TagNode<'db>,
    ) {
        // For now, just ignore intermediate tags like elif/else
        // In a full implementation, these would create new fragments/branches
    }

    fn handle_closer(
        &mut self,
        _tag_node: &TagNode<'db>,
        root_children: &mut Vec<SyntaxNodeId<'db>>,
    ) -> Result<(), ParserError> {
        if let Some(frame) = self.stack.pop() {
            // Create the complete block tag with its children
            let tag_specs = self.db.tag_specs();
            let name = frame.opener_tag.name.text(self.db);
            let spec = tag_specs.get(&name);
            let shape = spec.map_or(TagShape::Singleton, TagShape::from_spec);
            
            let meta = self.build_tag_meta(&frame.opener_tag, TagType::Opener, shape, spec);
            
            let syntax_node = SyntaxNode::Tag(crate::syntax_tree::TagNode {
                name: crate::syntax_tree::TagName::new(self.db, name.to_string()),
                bits: frame.opener_tag.bits.clone(),
                span: frame.opener_tag.span,
                meta,
                children: frame.children,
            });
            let node_id = SyntaxNodeId::new(self.db, syntax_node);
            
            self.add_to_current_container(node_id, root_children);
            Ok(())
        } else {
            Err(ParserError::InvalidSyntax {
                context: "Orphaned closing tag".to_string(),
            })
        }
    }

    fn handle_standalone(
        &mut self,
        tag_node: &TagNode<'db>,
        tag_specs: &TagSpecs,
        root_children: &mut Vec<SyntaxNodeId<'db>>,
    ) -> Result<(), ParserError> {
        let name = tag_node.name.text(self.db);
        let spec = tag_specs.get(&name);
        let meta = self.build_tag_meta(tag_node, TagType::Standalone, TagShape::Singleton, spec);
        
        let syntax_node = SyntaxNode::Tag(crate::syntax_tree::TagNode {
            name: crate::syntax_tree::TagName::new(self.db, name.to_string()),
            bits: tag_node.bits.clone(),
            span: tag_node.span,
            meta,
            children: Vec::new(),
        });
        let node_id = SyntaxNodeId::new(self.db, syntax_node);
        self.add_to_current_container(node_id, root_children);
        
        Ok(())
    }

    fn build_tag_meta(
        &self,
        tag_node: &TagNode<'db>,
        tag_type: TagType,
        shape: TagShape,
        spec: Option<&crate::templatetags::TagSpec>,
    ) -> TagMeta<'db> {
        let parsed_args = self.parse_arguments(&tag_node.bits, spec);
        
        TagMeta {
            tag_type,
            shape,
            spec_id: spec.and_then(|s| s.name.clone()),
            branch_kind: None,
            parsed_args,
        }
    }

    fn parse_arguments(
        &self,
        bits: &[String],
        spec: Option<&crate::templatetags::TagSpec>,
    ) -> ParsedArgs<'db> {
        let mut parsed_args = ParsedArgs::new();
        
        // If no spec is available, treat all bits as expressions
        let Some(spec) = spec else {
            for bit in bits {
                parsed_args.add_positional(ParsedArg::Expression(bit.clone()));
            }
            return parsed_args;
        };
        
        // Parse according to spec arguments
        let mut bit_index = 0;
        let mut positional_index = 0;
        
        while bit_index < bits.len() {
            let bit = &bits[bit_index];
            
            // Check for assignment (key=value)
            if let Some((key, value)) = bit.split_once('=') {
                parsed_args.add_named(
                    key.to_string(),
                    ParsedArg::Assignment {
                        name: key.to_string(),
                        value: value.to_string(),
                    },
                );
            } else {
                // Determine argument type based on spec
                let arg_type = spec.args.get(positional_index).map(|arg| &arg.arg_type);
                
                let parsed_arg = match arg_type {
                    Some(crate::templatetags::ArgType::Simple(simple_type)) => match simple_type {
                        crate::templatetags::SimpleArgType::Literal => {
                            ParsedArg::Literal(bit.clone())
                        }
                        crate::templatetags::SimpleArgType::Variable => {
                            ParsedArg::Variable(crate::syntax_tree::VariableName::new(self.db, bit.clone()))
                        }
                        crate::templatetags::SimpleArgType::String => {
                            ParsedArg::String(bit.clone())
                        }
                        crate::templatetags::SimpleArgType::Expression
                        | crate::templatetags::SimpleArgType::VarArgs => {
                            ParsedArg::Expression(bit.clone())
                        }
                        crate::templatetags::SimpleArgType::Assignment => ParsedArg::Assignment {
                            name: bit.clone(),
                            value: String::new(),
                        },
                    },
                    Some(crate::templatetags::ArgType::Choice { choice }) => {
                        // Validate against choices and treat as literal
                        if choice.contains(bit) {
                            ParsedArg::Literal(bit.clone())
                        } else {
                            // Invalid choice, treat as expression for error handling
                            ParsedArg::Expression(bit.clone())
                        }
                    }
                    None => {
                        // No more spec args, treat as expression
                        ParsedArg::Expression(bit.clone())
                    }
                };
                
                parsed_args.add_positional(parsed_arg);
                positional_index += 1;
            }
            
            bit_index += 1;
        }
        
        parsed_args
    }

    fn convert_node(&self, node: &Node<'db>) -> SyntaxNode<'db> {
        match node {
            Node::Text(text_node) => SyntaxNode::Text(crate::syntax_tree::TextNode {
                content: text_node.content.clone(),
                span: text_node.span,
            }),
            Node::Variable(var_node) => SyntaxNode::Variable(crate::syntax_tree::VariableNode {
                var: crate::syntax_tree::VariableName::new(self.db, var_node.var.text(self.db).to_string()),
                filters: var_node.filters.iter()
                    .map(|f| crate::syntax_tree::FilterName::new(self.db, f.text(self.db).to_string()))
                    .collect(),
                span: var_node.span,
            }),
            Node::Comment(comment_node) => SyntaxNode::Comment(crate::syntax_tree::CommentNode {
                content: comment_node.content.clone(),
                span: comment_node.span,
            }),
            Node::Tag(_) => unreachable!("Tag nodes should be handled separately"),
        }
    }

    fn add_to_current_container(
        &mut self,
        node_id: SyntaxNodeId<'db>,
        root_children: &mut Vec<SyntaxNodeId<'db>>,
    ) {
        if let Some(frame) = self.stack.last_mut() {
            // Add to current block's children
            frame.children.push(node_id);
        } else {
            // Add to root
            root_children.push(node_id);
        }
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
                    .unwrap_or_else(|_| crate::templatetags::TagSpecs::default())
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
        let tokens = Lexer::new(source).tokenize().unwrap();
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
            content: String,
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
                Node::Tag(TagNode { name, bits, span }) => TestNode::Tag {
                    name: name.text(db).to_string(),
                    bits: bits.clone(),
                    span: (span.start, span.length),
                },
                Node::Comment(CommentNode { content, span }) => TestNode::Comment {
                    content: content.clone(),
                    span: (span.start, span.length),
                },
                Node::Text(TextNode { content, span }) => TestNode::Text {
                    content: content.clone(),
                    span: (span.start, span.length),
                },
                Node::Variable(VariableNode { var, filters, span }) => TestNode::Variable {
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

    mod tree_builder {
        use super::*;

        #[salsa::tracked]
        fn test_simple_tree_building_impl(db: &dyn TemplateDb) -> (usize, String, String, usize) {
            let source = "Hello {{ user.name }}".to_string();
            let template = TestTemplate::new(db, source);
            let nodelist = parse_test_template(db, template);
            
            // Build syntax tree using the TreeBuilder
            let syntax_tree = crate::parser::build_syntax_tree(db, &nodelist).unwrap();
            
            // Verify the tree structure
            let root_children = syntax_tree.children(db);
            let children_count = root_children.len();
            
            // Check first child (Text)
            let first_content = match &root_children[0].resolve(db) {
                SyntaxNode::Text(text_node) => text_node.content.clone(),
                _ => panic!("Expected Text node"),
            };
            
            // Check second child (Variable)
            let (second_var, second_filters) = match &root_children[1].resolve(db) {
                SyntaxNode::Variable(var_node) => (
                    var_node.var.text(db).to_string(), 
                    var_node.filters.len()
                ),
                _ => panic!("Expected Variable node"),
            };
            
            (children_count, first_content, second_var, second_filters)
        }

        #[test]
        fn test_simple_tree_building() {
            let db = TestDatabase::new();
            let (children_count, first_content, second_var, second_filters) = test_simple_tree_building_impl(&db);
            
            assert_eq!(children_count, 2); // Text and Variable nodes
            assert_eq!(first_content, "Hello");
            assert_eq!(second_var, "user.name");
            assert_eq!(second_filters, 0);
        }

        #[salsa::tracked]
        fn test_if_block_tree_building_impl(db: &dyn TemplateDb) -> (usize, String, Vec<String>, bool, bool) {
            let source = "{% if user %}Hello {{ user.name }}{% endif %}".to_string();
            let template = TestTemplate::new(db, source);
            let nodelist = parse_test_template(db, template);
            

            
            // Build syntax tree using the TreeBuilder
            let syntax_tree = crate::parser::build_syntax_tree(db, &nodelist).unwrap();
            
            // Verify the tree structure
            let root_children = syntax_tree.children(db);
            let children_count = root_children.len();
            

            
            // Check the if tag
            match &root_children[0].resolve(db) {
                SyntaxNode::Tag(tag_node) => {
                    let name = tag_node.name.text(db).to_string();
                    let bits = tag_node.bits.clone();
                    let is_opener = matches!(tag_node.meta.tag_type, crate::templatetags::TagType::Opener);
                    let is_block = matches!(tag_node.meta.shape, crate::syntax_tree::TagShape::Block { .. });
                    (children_count, name, bits, is_opener, is_block)
                }
                _ => panic!("Expected Tag node"),
            }
        }

        #[test]
        fn test_if_block_tree_building() {
            let db = TestDatabase::new();
            let (children_count, name, bits, is_opener, is_block) = test_if_block_tree_building_impl(&db);
            
            assert_eq!(children_count, 1); // Only the if tag
            assert_eq!(name, "if");
            assert_eq!(bits, vec!["user"]);
            assert!(is_opener);
            assert!(is_block);
        }

        #[salsa::tracked]
        fn test_standalone_tag_tree_building_impl(db: &dyn TemplateDb) -> (usize, String, Vec<String>, bool, bool, usize) {
            let source = "{% load widget_tweaks %}".to_string();
            let template = TestTemplate::new(db, source);
            let nodelist = parse_test_template(db, template);
            
            // Build syntax tree using the TreeBuilder
            let syntax_tree = crate::parser::build_syntax_tree(db, &nodelist).unwrap();
            
            // Verify the tree structure
            let root_children = syntax_tree.children(db);
            let children_count = root_children.len();
            
            // Check the load tag
            match &root_children[0].resolve(db) {
                SyntaxNode::Tag(tag_node) => {
                    let name = tag_node.name.text(db).to_string();
                    let bits = tag_node.bits.clone();
                    let is_standalone = matches!(tag_node.meta.tag_type, crate::templatetags::TagType::Standalone);
                    let is_singleton = matches!(tag_node.meta.shape, crate::syntax_tree::TagShape::Singleton);
                    let child_count = tag_node.children.len();
                    (children_count, name, bits, is_standalone, is_singleton, child_count)
                }
                _ => panic!("Expected Tag node"),
            }
        }

        #[test]
        fn test_standalone_tag_tree_building() {
            let db = TestDatabase::new();
            let (children_count, name, bits, is_standalone, is_singleton, child_count) = test_standalone_tag_tree_building_impl(&db);
            
            assert_eq!(children_count, 1);
            assert_eq!(name, "load");
            assert_eq!(bits, vec!["widget_tweaks"]);
            assert!(is_standalone);
            assert!(is_singleton);
            assert_eq!(child_count, 0); // Standalone tags have no children
        }
    }
}

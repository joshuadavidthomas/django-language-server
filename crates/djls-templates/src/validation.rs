//! Django template validation.
//!
//! This module implements comprehensive validation for Django templates,
//! checking for proper tag matching, argument counts, and structural correctness.
//!
//! ## Validation Rules
//!
//! The validator checks for:
//! - Unclosed block tags (e.g., `{% if %}` without `{% endif %}`)
//! - Mismatched tag pairs (e.g., `{% if %}...{% endfor %}`)
//! - Orphaned intermediate tags (e.g., `{% else %}` without `{% if %}`)
//! - Invalid argument counts based on tag specifications
//! - Unmatched block names (e.g., `{% block content %}...{% endblock footer %}`)
//!
//! ## Architecture
//!
//! The `TagValidator` follows the same pattern as the Parser and Lexer,
//! maintaining minimal state and walking through the AST to accumulate errors.

use serde::Serialize;
use thiserror::Error;

use crate::ast::Node;
use crate::ast::NodeListError;
use crate::ast::Span;
use crate::ast::TagName;
use crate::ast::TagNode;
use crate::db::Db as TemplateDb;
use crate::syntax::SyntaxNode;
use crate::syntax::SyntaxNodeId;
use crate::syntax::SyntaxTree;
use crate::syntax::TagNode as SyntaxTagNode;
use crate::syntax::VariableNode as SyntaxVariableNode;
use crate::templatetags::Arg;
use crate::templatetags::ArgType;
use crate::templatetags::SimpleArgType;
use crate::templatetags::TagType;
use crate::NodeList;

#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize)]
pub enum SemanticError {
    #[error("Missing required argument '{arg_name}' for tag '{tag}' at position {span:?}")]
    MissingRequiredArg {
        tag: String,
        arg_name: String,
        span: Span,
    },
    
    #[error("Invalid argument type for '{arg_name}' in tag '{tag}': expected {expected}, found {found} at position {span:?}")]
    InvalidArgType {
        tag: String,
        arg_name: String,
        expected: String,
        found: String,
        span: Span,
    },
    
    #[error("Unknown argument '{arg_name}' for tag '{tag}' at position {span:?}")]
    UnknownArgument {
        tag: String,
        arg_name: String,
        span: Span,
    },
    
    #[error("Invalid choice value '{value}' for argument '{arg_name}' in tag '{tag}' at position {span:?}. Valid choices: {valid_choices:?}")]
    InvalidChoiceValue {
        tag: String,
        arg_name: String,
        value: String,
        valid_choices: Vec<String>,
        span: Span,
    },
    
    #[error("Too many positional arguments for tag '{tag}': maximum {max}, found {found} at position {span:?}")]
    TooManyPositionalArgs {
        tag: String,
        max: usize,
        found: usize,
        span: Span,
    },
    
    #[error("Conflicting arguments '{arg1}' and '{arg2}' for tag '{tag}' at position {span:?}")]
    ConflictingArgs {
        tag: String,
        arg1: String,
        arg2: String,
        span: Span,
    },
}

pub struct TagValidator<'db> {
    db: &'db dyn TemplateDb,
    ast: NodeList<'db>,
    current: usize,
    stack: Vec<TagNode<'db>>,
    errors: Vec<NodeListError>,
}

impl<'db> TagValidator<'db> {
    #[must_use]
    pub fn new(db: &'db dyn TemplateDb, ast: NodeList<'db>) -> Self {
        Self {
            db,
            ast,
            current: 0,
            stack: Vec::new(),
            errors: Vec::new(),
        }
    }

    #[must_use]
    pub fn validate(mut self) -> Vec<NodeListError> {
        while !self.is_at_end() {
            if let Some(Node::Tag(tag_node)) = self.current_node() {
                let TagNode { name, bits, span } = tag_node;
                let name_str = name.text(self.db);

                let tag_specs = self.db.tag_specs();
                let tag_type = TagType::for_name(&name_str, &tag_specs);

                let args = match tag_type {
                    TagType::Closer => tag_specs
                        .get_end_spec_for_closer(&name_str)
                        .map(|s| &s.args),
                    _ => tag_specs.get(&name_str).map(|s| &s.args),
                };

                self.check_arguments(&name_str, &bits, span, args);

                match tag_type {
                    TagType::Opener => {
                        self.stack.push(TagNode {
                            name,
                            bits: bits.clone(),
                            span,
                        });
                    }
                    TagType::Intermediate => {
                        self.handle_intermediate(&name_str, span);
                    }
                    TagType::Closer => {
                        self.handle_closer(name, &bits, span);
                    }
                    TagType::Standalone => {
                        // No additional action needed for standalone tags
                    }
                }
            }
            self.advance();
        }

        // Any remaining stack items are unclosed
        while let Some(tag) = self.stack.pop() {
            self.errors.push(NodeListError::UnclosedTag {
                tag: tag.name.text(self.db),
                span: tag.span,
            });
        }

        self.errors
    }

    fn check_arguments(
        &mut self,
        name: &str,
        bits: &[String],
        span: Span,
        args: Option<&Vec<Arg>>,
    ) {
        let Some(args) = args else {
            return;
        };

        // Count required arguments
        let required_count = args.iter().filter(|arg| arg.required).count();

        if bits.len() < required_count {
            self.errors.push(NodeListError::MissingRequiredArguments {
                tag: name.to_string(),
                min: required_count,
                span,
            });
        }

        // If there are more bits than defined args, that might be okay for varargs
        let has_varargs = args
            .iter()
            .any(|arg| matches!(arg.arg_type, ArgType::Simple(SimpleArgType::VarArgs)));

        if !has_varargs && bits.len() > args.len() {
            self.errors.push(NodeListError::TooManyArguments {
                tag: name.to_string(),
                max: args.len(),
                span,
            });
        }
    }

    fn handle_intermediate(&mut self, name: &str, span: Span) {
        // Check if this intermediate tag has the required parent
        let parent_tags = self.db.tag_specs().get_parent_tags_for_intermediate(name);
        if parent_tags.is_empty() {
            return; // Not an intermediate tag
        }

        // Check if any parent is in the stack
        let has_parent = self
            .stack
            .iter()
            .rev()
            .any(|tag| parent_tags.contains(&tag.name.text(self.db)));

        if !has_parent {
            let parents = if parent_tags.len() == 1 {
                parent_tags[0].clone()
            } else {
                parent_tags.join("' or '")
            };
            let context = format!("must appear within '{parents}' block");

            self.errors.push(NodeListError::OrphanedTag {
                tag: name.to_string(),
                context,
                span,
            });
        }
    }

    fn handle_closer(&mut self, name: TagName<'db>, bits: &[String], span: Span) {
        let name_str = name.text(self.db);

        if self.stack.is_empty() {
            // Stack is empty - unexpected closer
            self.errors.push(NodeListError::UnbalancedStructure {
                opening_tag: name_str.to_string(),
                expected_closing: String::new(),
                opening_span: span,
                closing_span: None,
            });
            return;
        }

        // Find the matching opener
        let expected_opener = self.db.tag_specs().find_opener_for_closer(&name_str);
        let Some(opener_name) = expected_opener else {
            // Unknown closer
            self.errors.push(NodeListError::UnbalancedStructure {
                opening_tag: name_str.to_string(),
                expected_closing: String::new(),
                opening_span: span,
                closing_span: None,
            });
            return;
        };

        // Find matching opener in stack
        let found_index = if bits.is_empty() {
            // Unnamed closer - find nearest opener
            self.stack
                .iter()
                .enumerate()
                .rev()
                .find(|(_, tag)| tag.name.text(self.db) == opener_name)
                .map(|(i, _)| i)
        } else {
            // Named closer - try to find exact match
            self.stack
                .iter()
                .enumerate()
                .rev()
                .find(|(_, tag)| {
                    tag.name.text(self.db) == opener_name
                        && !tag.bits.is_empty()
                        && tag.bits[0] == bits[0]
                })
                .map(|(i, _)| i)
        };

        if let Some(index) = found_index {
            // Found a match - pop everything after as unclosed
            self.pop_unclosed_after(index);

            // Remove the matched tag
            if bits.is_empty() {
                self.stack.pop();
            } else {
                self.stack.remove(index);
            }
        } else if !bits.is_empty() {
            // Named closer with no matching named block
            // Report the mismatch
            self.errors.push(NodeListError::UnmatchedBlockName {
                name: bits[0].clone(),
                span,
            });

            // Find the nearest block to close (and report it as unclosed)
            if let Some((index, nearest_block)) = self
                .stack
                .iter()
                .enumerate()
                .rev()
                .find(|(_, tag)| tag.name.text(self.db) == opener_name)
            {
                // Report that we're closing the wrong block
                self.errors.push(NodeListError::UnclosedTag {
                    tag: nearest_block.name.text(self.db),
                    span: nearest_block.span,
                });

                // Pop everything after as unclosed
                self.pop_unclosed_after(index);

                // Remove the block we're erroneously closing
                self.stack.pop();
            }
        } else {
            // No opener found at all
            self.errors.push(NodeListError::UnbalancedStructure {
                opening_tag: opener_name,
                expected_closing: name_str.to_string(),
                opening_span: span,
                closing_span: None,
            });
        }
    }

    fn pop_unclosed_after(&mut self, index: usize) {
        while self.stack.len() > index + 1 {
            if let Some(unclosed) = self.stack.pop() {
                self.errors.push(NodeListError::UnclosedTag {
                    tag: unclosed.name.text(self.db),
                    span: unclosed.span,
                });
            }
        }
    }

    fn current_node(&self) -> Option<Node<'db>> {
        self.ast.nodelist(self.db).get(self.current).cloned()
    }

    fn advance(&mut self) {
        self.current += 1;
    }

    fn is_at_end(&self) -> bool {
        self.current >= self.ast.nodelist(self.db).len()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::templatetags::TagSpecs;
    use crate::Lexer;
    use crate::Parser;

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
            let toml_str = include_str!("../tagspecs/django.toml");
            Arc::new(TagSpecs::from_toml(toml_str).unwrap())
        }
    }

    #[salsa::input]
    struct TestSource {
        #[returns(ref)]
        text: String,
    }

    #[salsa::tracked]
    fn parse_test_template(db: &dyn TemplateDb, source: TestSource) -> NodeList<'_> {
        let text = source.text(db);
        let (tokens, _) = Lexer::new(text).tokenize();
        let token_stream = crate::tokens::TokenStream::new(db, tokens);
        let mut parser = Parser::new(db, token_stream);
        let (ast, _) = parser.parse().unwrap();
        ast
    }

    #[test]
    fn test_match_simple_if_endif() {
        let db = TestDatabase::new();
        let source = TestSource::new(&db, "{% if x %}content{% endif %}".to_string());
        let ast = parse_test_template(&db, source);
        let errors = TagValidator::new(&db, ast).validate();
        assert!(errors.is_empty());
    }

    #[test]
    fn test_unclosed_if() {
        let db = TestDatabase::new();
        let source = TestSource::new(&db, "{% if x %}content".to_string());
        let ast = parse_test_template(&db, source);
        let errors = TagValidator::new(&db, ast).validate();
        assert_eq!(errors.len(), 1);
        match &errors[0] {
            NodeListError::UnclosedTag { tag, .. } => assert_eq!(tag, "if"),
            _ => panic!("Expected UnclosedTag error"),
        }
    }

    #[test]
    fn test_mismatched_tags() {
        let db = TestDatabase::new();
        let source = TestSource::new(&db, "{% if x %}content{% endfor %}".to_string());
        let ast = parse_test_template(&db, source);
        let errors = TagValidator::new(&db, ast).validate();
        assert!(!errors.is_empty());
        // Should have unexpected closer for endfor and unclosed for if
    }

    #[test]
    fn test_orphaned_else() {
        let db = TestDatabase::new();
        let source = TestSource::new(&db, "{% else %}content".to_string());
        let ast = parse_test_template(&db, source);
        let errors = TagValidator::new(&db, ast).validate();
        assert_eq!(errors.len(), 1);
        match &errors[0] {
            NodeListError::OrphanedTag { tag, .. } => assert_eq!(tag, "else"),
            _ => panic!("Expected OrphanedTag error"),
        }
    }

    #[test]
    fn test_nested_blocks() {
        let db = TestDatabase::new();
        let source = TestSource::new(
            &db,
            "{% if x %}{% for i in items %}{{ i }}{% endfor %}{% endif %}".to_string(),
        );
        let ast = parse_test_template(&db, source);
        let errors = TagValidator::new(&db, ast).validate();
        assert!(errors.is_empty());
    }

    #[test]
    fn test_complex_if_elif_else() {
        let db = TestDatabase::new();
        let source = TestSource::new(
            &db,
            "{% if x %}a{% elif y %}b{% else %}c{% endif %}".to_string(),
        );
        let ast = parse_test_template(&db, source);
        let errors = TagValidator::new(&db, ast).validate();
        assert!(errors.is_empty());
    }

    #[test]
    fn test_missing_required_arguments() {
        let db = TestDatabase::new();
        let source = TestSource::new(&db, "{% load %}".to_string());
        let ast = parse_test_template(&db, source);
        let errors = TagValidator::new(&db, ast).validate();
        assert!(!errors.is_empty());
        assert!(errors
            .iter()
            .any(|e| matches!(e, NodeListError::MissingRequiredArguments { .. })));
    }

    #[test]
    fn test_unnamed_endblock_closes_nearest_block() {
        let db = TestDatabase::new();
        let source = TestSource::new(&db, "{% block outer %}{% if x %}{% block inner %}test{% endblock %}{% endif %}{% endblock %}".to_string());
        let ast = parse_test_template(&db, source);
        let errors = TagValidator::new(&db, ast).validate();
        assert!(errors.is_empty());
    }

    #[test]
    fn test_named_endblock_matches_named_block() {
        let db = TestDatabase::new();
        let source = TestSource::new(
            &db,
            "{% block content %}{% if x %}test{% endif %}{% endblock content %}".to_string(),
        );
        let ast = parse_test_template(&db, source);
        let errors = TagValidator::new(&db, ast).validate();
        assert!(errors.is_empty());
    }

    #[test]
    fn test_mismatched_block_names() {
        let db = TestDatabase::new();
        let source = TestSource::new(
            &db,
            "{% block content %}test{% endblock footer %}".to_string(),
        );
        let ast = parse_test_template(&db, source);
        let errors = TagValidator::new(&db, ast).validate();
        assert!(!errors.is_empty());
        assert!(errors
            .iter()
            .any(|e| matches!(e, NodeListError::UnmatchedBlockName { .. })));
    }

    #[test]
    fn test_unclosed_tags_with_unnamed_endblock() {
        let db = TestDatabase::new();
        let source = TestSource::new(
            &db,
            "{% block content %}{% if x %}test{% endblock %}".to_string(),
        );
        let ast = parse_test_template(&db, source);
        let errors = TagValidator::new(&db, ast).validate();
        assert!(!errors.is_empty());
        assert!(errors
            .iter()
            .any(|e| matches!(e, NodeListError::UnclosedTag { tag, .. } if tag == "if")));
    }
}

/// New hierarchical validation using `SyntaxTree` structure.
pub struct SyntaxTreeValidator<'db> {
    db: &'db dyn TemplateDb,
    tree: SyntaxTree<'db>,
    errors: Vec<NodeListError>,
}

impl<'db> SyntaxTreeValidator<'db> {
    #[must_use]
    pub fn new(db: &'db dyn TemplateDb, tree: SyntaxTree<'db>) -> Self {
        Self {
            db,
            tree,
            errors: Vec::new(),
        }
    }

    #[must_use]
    pub fn validate(mut self) -> Vec<NodeListError> {
        self.validate_node(self.tree.root(self.db));
        self.errors
    }

    fn validate_node(&mut self, node_id: SyntaxNodeId<'db>) {
        let node = node_id.resolve(self.db);

        match &node {
            SyntaxNode::Root { children } => {
                for child_id in children {
                    self.validate_node(*child_id);
                }
            }
            SyntaxNode::Tag(tag_node) => {
                self.validate_tag_node(node_id, tag_node);
                for child_id in &tag_node.children {
                    self.validate_node(*child_id);
                }
            }
            SyntaxNode::Variable(var_node) => {
                self.validate_variable_node(var_node);
            }
            SyntaxNode::Text(_) | SyntaxNode::Comment(_) => {}
            SyntaxNode::Error { message, span } => {
                self.errors.push(NodeListError::InvalidNode {
                    node_type: "Error".to_string(),
                    reason: message.clone(),
                    span: *span,
                });
            }
        }
    }

    fn validate_tag_node(&mut self, node_id: SyntaxNodeId<'db>, tag_node: &SyntaxTagNode<'db>) {
        let name_str = tag_node.name.text(self.db);
        self.validate_tag_arguments(&name_str, tag_node);

        if tag_node.meta.tag_type == TagType::Intermediate {
            self.validate_intermediate_tag(node_id, tag_node);
        }

        // Check for unclosed blocks
        if tag_node.meta.unclosed {
            self.errors.push(NodeListError::UnclosedTag {
                tag: name_str.clone(),
                span: tag_node.span,
            });
        }
    }

    fn validate_tag_arguments(&mut self, name: &str, tag_node: &SyntaxTagNode<'db>) {
        let tag_specs = self.db.tag_specs();
        let Some(args) = tag_specs.get(name).map(|s| &s.args) else {
            return;
        };
        let parsed_args = &tag_node.meta.parsed_args;
        let required_count = args.iter().filter(|arg| arg.required).count();

        if parsed_args.positional_count() < required_count {
            self.errors.push(NodeListError::MissingRequiredArguments {
                tag: name.to_string(),
                min: required_count,
                span: tag_node.span,
            });
        }
    }

    fn validate_intermediate_tag(
        &mut self,
        node_id: SyntaxNodeId<'db>,
        tag_node: &SyntaxTagNode<'db>,
    ) {
        let name_str = tag_node.name.text(self.db);
        let parent_tags = self
            .db
            .tag_specs()
            .get_parent_tags_for_intermediate(&name_str);
        if parent_tags.is_empty() {
            return;
        }

        if let Some(parent_block) = self.find_parent_block(node_id) {
            if let SyntaxNode::Tag(parent_tag) = parent_block.resolve(self.db) {
                let parent_name = parent_tag.name.text(self.db);
                if !parent_tags.contains(&parent_name) {
                    let parents = if parent_tags.len() == 1 {
                        parent_tags[0].clone()
                    } else {
                        parent_tags.join("' or '")
                    };
                    let context = format!("must appear within '{parents}' block");

                    self.errors.push(NodeListError::OrphanedTag {
                        tag: name_str.clone(),
                        context,
                        span: tag_node.span,
                    });
                }
            }
        } else {
            let parents = if parent_tags.len() == 1 {
                parent_tags[0].clone()
            } else {
                parent_tags.join("' or '")
            };
            let context = format!("must appear within '{parents}' block");

            self.errors.push(NodeListError::OrphanedTag {
                tag: name_str.clone(),
                context,
                span: tag_node.span,
            });
        }
    }

    #[allow(clippy::unused_self)]
    fn validate_variable_node(&mut self, _var_node: &SyntaxVariableNode<'db>) {
        // Variable validation could be added here in the future
        // For now, variables are always considered valid
    }

    fn find_parent_block(&self, node_id: SyntaxNodeId<'db>) -> Option<SyntaxNodeId<'db>> {
        let mut current = node_id;
        while let Some(parent) = current.find_parent(self.db, &self.tree) {
            if let SyntaxNode::Tag(parent_tag) = parent.resolve(self.db) {
                if parent_tag.meta.can_have_children() {
                    return Some(parent);
                }
            }
            current = parent;
        }
        None
    }
}

#[cfg(test)]
mod syntax_tree_tests {
    use std::sync::Arc;

    use super::*;
    use crate::syntax::SyntaxTree;
    use crate::Lexer;
    use crate::Parser;

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
            let toml_str = include_str!("../tagspecs/django.toml");
            Arc::new(crate::templatetags::TagSpecs::from_toml(toml_str).unwrap())
        }
    }

    #[salsa::input]
    struct TestSource {
        #[returns(ref)]
        text: String,
    }

    #[salsa::tracked]
    fn parse_test_syntax_tree(db: &dyn TemplateDb, source: TestSource) -> SyntaxTree<'_> {
        let text = source.text(db);
        let (tokens, _) = Lexer::new(text).tokenize();
        let token_stream = crate::tokens::TokenStream::new(db, tokens);
        let mut parser = Parser::new(db, token_stream);
        let (nodelist, _) = parser.parse().unwrap();
        crate::build_syntax_tree_inline(db, nodelist)
    }

    #[test]
    fn test_syntax_tree_validation_simple_if() {
        let db = TestDatabase::new();
        let source = TestSource::new(&db, "{% if x %}content{% endif %}".to_string());

        let syntax_tree = parse_test_syntax_tree(&db, source);
        let errors = SyntaxTreeValidator::new(&db, syntax_tree).validate();
        assert!(
            errors.is_empty(),
            "Simple if should have no errors, got: {errors:#?}"
        );
    }

    #[test]
    fn test_syntax_tree_validation_orphaned_else() {
        let db = TestDatabase::new();
        let source = TestSource::new(&db, "{% else %}content".to_string());

        let syntax_tree = parse_test_syntax_tree(&db, source);
        let errors = SyntaxTreeValidator::new(&db, syntax_tree).validate();

        assert!(!errors.is_empty(), "Orphaned else should produce errors");
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, NodeListError::OrphanedTag { tag, .. } if tag == "else")),
            "Should have orphaned tag error for else: {errors:#?}"
        );
    }

    #[test]
    fn test_syntax_tree_validation_missing_required_args() {
        let db = TestDatabase::new();
        let source = TestSource::new(&db, "{% load %}".to_string());

        let syntax_tree = parse_test_syntax_tree(&db, source);
        let errors = SyntaxTreeValidator::new(&db, syntax_tree).validate();

        assert!(
            !errors.is_empty(),
            "Load without args should produce errors"
        );
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, NodeListError::MissingRequiredArguments { .. })),
            "Should have missing required arguments error: {errors:#?}"
        );
    }

    #[test]
    fn test_syntax_tree_raw_blocks() {
        let db = TestDatabase::new();

        // Test comment block
        let source = TestSource::new(
            &db,
            "{% comment %}This is a comment{% endcomment %}".to_string(),
        );
        let syntax_tree = parse_test_syntax_tree(&db, source);
        let errors = SyntaxTreeValidator::new(&db, syntax_tree).validate();
        assert!(
            errors.is_empty(),
            "Comment block should have no errors, got: {errors:#?}"
        );

        // Test verbatim block
        let source = TestSource::new(
            &db,
            "{% verbatim %}{{ raw_content }}{% endverbatim %}".to_string(),
        );
        let syntax_tree = parse_test_syntax_tree(&db, source);
        let errors = SyntaxTreeValidator::new(&db, syntax_tree).validate();
        assert!(
            errors.is_empty(),
            "Verbatim block should have no errors, got: {errors:#?}"
        );

        // Test spaceless block
        let source = TestSource::new(
            &db,
            "{% spaceless %}<p>  content  </p>{% endspaceless %}".to_string(),
        );
        let syntax_tree = parse_test_syntax_tree(&db, source);
        let errors = SyntaxTreeValidator::new(&db, syntax_tree).validate();
        assert!(
            errors.is_empty(),
            "Spaceless block should have no errors, got: {errors:#?}"
        );
    }

    #[test]
    fn test_syntax_tree_for_empty() {
        let db = TestDatabase::new();
        let source = TestSource::new(
            &db,
            "{% for item in items %}{{ item }}{% empty %}No items{% endfor %}".to_string(),
        );

        let syntax_tree = parse_test_syntax_tree(&db, source);
        let errors = SyntaxTreeValidator::new(&db, syntax_tree).validate();
        assert!(
            errors.is_empty(),
            "For/empty should have no errors, got: {errors:#?}"
        );
    }

    #[test]
    fn test_syntax_tree_complex_nested_structures() {
        let db = TestDatabase::new();

        // Test deeply nested if/for blocks
        let source = TestSource::new(&db,
            "{% if user %}{% for item in items %}{% if item.active %}{{ item.name }}{% endif %}{% endfor %}{% endif %}".to_string());
        let syntax_tree = parse_test_syntax_tree(&db, source);
        let errors = SyntaxTreeValidator::new(&db, syntax_tree).validate();
        assert!(
            errors.is_empty(),
            "Nested structures should have no errors, got: {errors:#?}"
        );

        // Test multiple elif branches
        let source = TestSource::new(&db,
            "{% if x == 1 %}one{% elif x == 2 %}two{% elif x == 3 %}three{% else %}other{% endif %}".to_string());
        let syntax_tree = parse_test_syntax_tree(&db, source);
        let errors = SyntaxTreeValidator::new(&db, syntax_tree).validate();
        assert!(
            errors.is_empty(),
            "Multiple elif branches should have no errors, got: {errors:#?}"
        );

        // Test mixed content in branches
        let source = TestSource::new(&db,
            "{% if x %}Text {{ var }}{% for i in list %}{{ i }}{% endfor %}{% else %}Nothing{% endif %}".to_string());
        let syntax_tree = parse_test_syntax_tree(&db, source);
        let errors = SyntaxTreeValidator::new(&db, syntax_tree).validate();
        assert!(
            errors.is_empty(),
            "Mixed content in branches should have no errors, got: {errors:#?}"
        );
    }

    #[test]
    fn test_syntax_tree_orphaned_intermediate_tags() {
        let db = TestDatabase::new();

        // Test orphaned empty without for
        let source = TestSource::new(&db, "{% empty %}No items".to_string());
        let syntax_tree = parse_test_syntax_tree(&db, source);
        let errors = SyntaxTreeValidator::new(&db, syntax_tree).validate();
        assert!(!errors.is_empty(), "Orphaned empty should produce errors");
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, NodeListError::OrphanedTag { tag, .. } if tag == "empty")),
            "Should have orphaned tag error for empty: {errors:#?}"
        );

        // Test elif without if
        let source = TestSource::new(&db, "{% elif x %}content".to_string());
        let syntax_tree = parse_test_syntax_tree(&db, source);
        let errors = SyntaxTreeValidator::new(&db, syntax_tree).validate();
        assert!(!errors.is_empty(), "Orphaned elif should produce errors");
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, NodeListError::OrphanedTag { tag, .. } if tag == "elif")),
            "Should have orphaned tag error for elif: {errors:#?}"
        );
    }

    #[test]
    fn test_syntax_tree_unclosed_blocks_detection() {
        let db = TestDatabase::new();

        // Test unclosed if
        let source = TestSource::new(&db, "{% if x %}content".to_string());
        let syntax_tree = parse_test_syntax_tree(&db, source);
        let errors = SyntaxTreeValidator::new(&db, syntax_tree).validate();
        assert!(!errors.is_empty(), "Unclosed if should produce errors");
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, NodeListError::UnclosedTag { tag, .. } if tag == "if")),
            "Should have unclosed tag error for if: {errors:#?}"
        );

        // Test unclosed for
        let source = TestSource::new(&db, "{% for item in items %}{{ item }}".to_string());
        let syntax_tree = parse_test_syntax_tree(&db, source);
        let errors = SyntaxTreeValidator::new(&db, syntax_tree).validate();
        assert!(!errors.is_empty(), "Unclosed for should produce errors");
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, NodeListError::UnclosedTag { tag, .. } if tag == "for")),
            "Should have unclosed tag error for for: {errors:#?}"
        );
    }

    #[test]
    fn test_tree_vs_flat_validation_equivalence() {
        let test_cases = vec![
            "{% if x %}content{% endif %}",
            "{% if x %}content", // unclosed
            "{% else %}content", // orphaned
            "{% load %}",        // missing args
            "{% if x %}{% else %}{% endif %}",
            "{% for i in items %}{{ i }}{% endfor %}",
        ];

        let db = TestDatabase::new();

        for template in test_cases {
            let source = TestSource::new(&db, template.to_string());

            // Get errors from flat validator
            let ast = parse_test_template(&db, source);
            let flat_errors = super::TagValidator::new(&db, ast).validate();

            // Get errors from tree validator
            let syntax_tree = parse_test_syntax_tree(&db, source);
            let tree_errors = SyntaxTreeValidator::new(&db, syntax_tree).validate();

            // For now, we check that both produce errors or both don't
            // Full equivalence would require mapping error details
            assert_eq!(
                flat_errors.is_empty(),
                tree_errors.is_empty(),
                "Validation mismatch for template: {}\nFlat errors: {:?}\nTree errors: {:?}",
                template,
                flat_errors.len(),
                tree_errors.len()
            );
        }
    }

    #[salsa::tracked]
    fn parse_test_template(db: &dyn TemplateDb, source: TestSource) -> crate::NodeList<'_> {
        let text = source.text(db);
        let (tokens, _) = Lexer::new(text).tokenize();
        let token_stream = crate::tokens::TokenStream::new(db, tokens);
        let mut parser = Parser::new(db, token_stream);
        let (ast, _) = parser.parse().unwrap();
        ast
    }
}

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

use crate::ast::AstError;
use crate::ast::Node;
use crate::ast::Span;
use crate::ast::TagName;
use crate::ast::TagNode;
use crate::db::Db as TemplateDb;
use crate::templatetags::TagType;
use crate::Ast;

pub struct TagValidator<'db> {
    db: &'db dyn TemplateDb,
    ast: Ast<'db>,
    current: usize,
    stack: Vec<TagNode<'db>>,
    errors: Vec<AstError>,
}

impl<'db> TagValidator<'db> {
    #[must_use]
    pub fn new(db: &'db dyn TemplateDb, ast: Ast<'db>) -> Self {
        Self {
            db,
            ast,
            current: 0,
            stack: Vec::new(),
            errors: Vec::new(),
        }
    }

    #[must_use]
    pub fn validate(mut self) -> Vec<AstError> {
        while !self.is_at_end() {
            if let Some(Node::Tag { name, bits, span }) = self.current_node() {
                let name_str = name.text(self.db);
                self.check_arguments(&name_str, &bits, span);

                match TagType::for_name(&name_str, &self.db.tag_specs()) {
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
                        // Standalone tags don't need special handling
                    }
                }
            }
            self.advance();
        }

        // Any remaining stack items are unclosed
        while let Some(tag) = self.stack.pop() {
            self.errors.push(AstError::UnclosedTag {
                tag: tag.name.text(self.db),
                span_start: tag.span.start(self.db),
                span_length: tag.span.length(self.db),
            });
        }

        self.errors
    }

    fn check_arguments(&mut self, name: &str, bits: &[String], span: Span<'db>) {
        if let Some(spec) = self.db.tag_specs().get(name) {
            if let Some(arg_spec) = &spec.args {
                if let Some(min) = arg_spec.min {
                    if bits.len() < min {
                        self.errors.push(AstError::MissingRequiredArguments {
                            tag: name.to_string(),
                            min,
                            span_start: span.start(self.db),
                            span_length: span.length(self.db),
                        });
                    }
                }
                if let Some(max) = arg_spec.max {
                    if bits.len() > max {
                        self.errors.push(AstError::TooManyArguments {
                            tag: name.to_string(),
                            max,
                            span_start: span.start(self.db),
                            span_length: span.length(self.db),
                        });
                    }
                }
            }
        }
    }

    fn handle_intermediate(&mut self, name: &str, span: Span<'db>) {
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
            let context = if parent_tags.len() == 1 {
                let parent = &parent_tags[0];
                if parent == "if" {
                    if name == "elif" {
                        "must appear after 'if' or another 'elif'".to_string()
                    } else {
                        format!("must appear within '{parent}' block")
                    }
                } else {
                    format!("must appear within '{parent}' block")
                }
            } else {
                let parents = parent_tags.join("' or '");
                format!("must appear within '{parents}' block")
            };

            self.errors.push(AstError::OrphanedTag {
                tag: name.to_string(),
                context,
                span_start: span.start(self.db),
                span_length: span.length(self.db),
            });
        }
    }

    fn handle_closer(&mut self, name: TagName<'db>, bits: &[String], span: Span<'db>) {
        let name_str = name.text(self.db);
        // Special handling for endblock
        if name_str == "endblock" {
            if bits.is_empty() {
                // Unnamed endblock - find nearest block
                let mut found_index = None;

                for (i, tag) in self.stack.iter().enumerate().rev() {
                    if tag.name.text(self.db) == "block" {
                        found_index = Some(i);
                        break;
                    }
                }

                if let Some(index) = found_index {
                    // Mark everything between as unclosed
                    while self.stack.len() > index + 1 {
                        if let Some(unclosed) = self.stack.pop() {
                            self.errors.push(AstError::UnclosedTag {
                                tag: unclosed.name.text(self.db),
                                span_start: unclosed.span.start(self.db),
                                span_length: unclosed.span.length(self.db),
                            });
                        }
                    }
                    // Pop the block
                    self.stack.pop();
                } else {
                    // No block found for endblock
                    self.errors.push(AstError::UnbalancedStructure {
                        opening_tag: name.text(self.db),
                        expected_closing: "block".to_string(),
                        opening_span_start: span.start(self.db),

                        opening_span_length: span.length(self.db),
                        closing_span_start: None,
                        closing_span_length: None,
                    });
                }
            } else {
                // Named endblock - find matching block with same name
                let target_name = &bits[0];
                let mut found_index = None;

                for (i, tag) in self.stack.iter().enumerate().rev() {
                    if tag.name.text(self.db) == "block"
                        && !tag.bits.is_empty()
                        && tag.bits[0] == *target_name
                    {
                        found_index = Some(i);
                        break;
                    }
                }

                if let Some(index) = found_index {
                    // Mark everything after the target as unclosed
                    while self.stack.len() > index + 1 {
                        if let Some(unclosed) = self.stack.pop() {
                            self.errors.push(AstError::UnclosedTag {
                                tag: unclosed.name.text(self.db),
                                span_start: unclosed.span.start(self.db),
                                span_length: unclosed.span.length(self.db),
                            });
                        }
                    }
                    // Pop the matching block
                    self.stack.remove(index);
                } else {
                    // No matching block found
                    self.errors.push(AstError::UnmatchedBlockName {
                        name: target_name.clone(),
                        span_start: span.start(self.db),
                        span_length: span.length(self.db),
                    });
                }
            }
        } else if self.stack.is_empty() {
            // Stack is empty - unexpected closer
            self.errors.push(AstError::UnbalancedStructure {
                opening_tag: name.text(self.db),
                expected_closing: String::new(),
                opening_span_start: span.start(self.db),

                opening_span_length: span.length(self.db),
                closing_span_start: None,
                closing_span_length: None,
            });
        } else {
            // Find the matching opener
            let expected_opener = self.db.tag_specs().find_opener_for_closer(&name_str);
            if let Some(opener_name) = expected_opener {
                // Search for matching opener
                let mut found_index = None;
                for (i, tag) in self.stack.iter().enumerate().rev() {
                    if tag.name.text(self.db) == opener_name {
                        found_index = Some(i);
                        break;
                    }
                }

                if let Some(index) = found_index {
                    // Mark everything after the opener as unclosed
                    while self.stack.len() > index + 1 {
                        if let Some(unclosed) = self.stack.pop() {
                            self.errors.push(AstError::UnclosedTag {
                                tag: unclosed.name.text(self.db),
                                span_start: unclosed.span.start(self.db),
                                span_length: unclosed.span.length(self.db),
                            });
                        }
                    }
                    // Pop the matching opener
                    self.stack.pop();
                } else {
                    // No matching opener found
                    self.errors.push(AstError::UnbalancedStructure {
                        opening_tag: opener_name,
                        expected_closing: name.text(self.db),
                        opening_span_start: span.start(self.db),

                        opening_span_length: span.length(self.db),
                        closing_span_start: None,
                        closing_span_length: None,
                    });
                }
            } else {
                // Unknown closer
                self.errors.push(AstError::UnbalancedStructure {
                    opening_tag: name.text(self.db),
                    expected_closing: String::new(),
                    opening_span_start: span.start(self.db),

                    opening_span_length: span.length(self.db),
                    closing_span_start: None,
                    closing_span_length: None,
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
    fn parse_test_template(db: &dyn TemplateDb, source: TestSource) -> Ast<'_> {
        let text = source.text(db);
        let tokens = Lexer::new(text).tokenize().unwrap();
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
            AstError::UnclosedTag { tag, .. } => assert_eq!(tag, "if"),
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
            AstError::OrphanedTag { tag, .. } => assert_eq!(tag, "else"),
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
            .any(|e| matches!(e, AstError::MissingRequiredArguments { .. })));
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
            .any(|e| matches!(e, AstError::UnmatchedBlockName { .. })));
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
            .any(|e| matches!(e, AstError::UnclosedTag { tag, .. } if tag == "if")));
    }
}

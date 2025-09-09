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
use crate::templatetags::Arg;
use crate::templatetags::ArgType;
use crate::templatetags::SimpleArgType;
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
            self.errors.push(AstError::UnclosedTag {
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
            self.errors.push(AstError::MissingRequiredArguments {
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
            self.errors.push(AstError::TooManyArguments {
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

            self.errors.push(AstError::OrphanedTag {
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
            self.errors.push(AstError::UnbalancedStructure {
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
            self.errors.push(AstError::UnbalancedStructure {
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
            self.errors.push(AstError::UnmatchedBlockName {
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
                self.errors.push(AstError::UnclosedTag {
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
            self.errors.push(AstError::UnbalancedStructure {
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
                self.errors.push(AstError::UnclosedTag {
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

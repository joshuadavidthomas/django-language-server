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

use std::sync::Arc;

use crate::ast::AstError;
use crate::ast::Node;
use crate::ast::Span;
use crate::ast::TagNode;
use crate::templatetags::TagSpecs;
use crate::templatetags::TagType;
use crate::Ast;

pub struct TagValidator {
    ast: Ast,
    current: usize,
    tag_specs: Arc<TagSpecs>,
    stack: Vec<TagNode>,
    errors: Vec<AstError>,
}

impl TagValidator {
    #[must_use]
    pub fn new(ast: Ast, tag_specs: Arc<TagSpecs>) -> Self {
        Self {
            ast,
            current: 0,
            tag_specs,
            stack: Vec::new(),
            errors: Vec::new(),
        }
    }

    #[must_use]
    pub fn validate(mut self) -> Vec<AstError> {
        while !self.is_at_end() {
            if let Some(Node::Tag { name, bits, span }) = self.current_node() {
                // Clone the data we need before mutating self
                let name = name.clone();
                let bits = bits.clone();
                let span = *span;

                self.check_arguments(&name, &bits, span);

                match TagType::for_name(&name, &self.tag_specs) {
                    TagType::Opener => {
                        self.stack.push(TagNode {
                            name: name.clone(),
                            bits: bits.clone(),
                            span,
                        });
                    }
                    TagType::Intermediate => {
                        self.handle_intermediate(&name, span);
                    }
                    TagType::Closer => {
                        self.handle_closer(&name, &bits, span);
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
                tag: tag.name,
                span: tag.span,
            });
        }

        self.errors
    }

    fn check_arguments(&mut self, name: &str, bits: &[String], span: Span) {
        if let Some(spec) = self.tag_specs.get(name) {
            if let Some(arg_spec) = &spec.args {
                if let Some(min) = arg_spec.min {
                    if bits.len() < min {
                        self.errors.push(AstError::MissingRequiredArguments {
                            tag: name.to_string(),
                            min,
                            span,
                        });
                    }
                }
                if let Some(max) = arg_spec.max {
                    if bits.len() > max {
                        self.errors.push(AstError::TooManyArguments {
                            tag: name.to_string(),
                            max,
                            span,
                        });
                    }
                }
            }
        }
    }

    fn handle_intermediate(&mut self, name: &str, span: Span) {
        // Check if this intermediate tag has the required parent
        let parent_tags = self.tag_specs.get_parent_tags_for_intermediate(name);
        if parent_tags.is_empty() {
            return; // Not an intermediate tag
        }

        // Check if any parent is in the stack
        let has_parent = self
            .stack
            .iter()
            .rev()
            .any(|tag| parent_tags.contains(&tag.name));

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
                span,
            });
        }
    }

    fn handle_closer(&mut self, name: &str, bits: &[String], span: Span) {
        // Special handling for endblock
        if name == "endblock" {
            if bits.is_empty() {
                // Unnamed endblock - find nearest block
                let mut found_index = None;

                for (i, tag) in self.stack.iter().enumerate().rev() {
                    if tag.name == "block" {
                        found_index = Some(i);
                        break;
                    }
                }

                if let Some(index) = found_index {
                    // Mark everything between as unclosed
                    while self.stack.len() > index + 1 {
                        if let Some(unclosed) = self.stack.pop() {
                            self.errors.push(AstError::UnclosedTag {
                                tag: unclosed.name,
                                span: unclosed.span,
                            });
                        }
                    }
                    // Pop the block
                    self.stack.pop();
                } else {
                    // No block found for endblock
                    self.errors.push(AstError::UnbalancedStructure {
                        opening_tag: name.to_string(),
                        expected_closing: "block".to_string(),
                        opening_span: span,
                        closing_span: None,
                    });
                }
            } else {
                // Named endblock - find matching block with same name
                let target_name = &bits[0];
                let mut found_index = None;

                for (i, tag) in self.stack.iter().enumerate().rev() {
                    if tag.name == "block" && !tag.bits.is_empty() && tag.bits[0] == *target_name {
                        found_index = Some(i);
                        break;
                    }
                }

                if let Some(index) = found_index {
                    // Mark everything after the target as unclosed
                    while self.stack.len() > index + 1 {
                        if let Some(unclosed) = self.stack.pop() {
                            self.errors.push(AstError::UnclosedTag {
                                tag: unclosed.name,
                                span: unclosed.span,
                            });
                        }
                    }
                    // Pop the matching block
                    self.stack.remove(index);
                } else {
                    // No matching block found
                    self.errors.push(AstError::UnmatchedBlockName {
                        name: target_name.clone(),
                        span,
                    });
                }
            }
        } else if self.stack.is_empty() {
            // Stack is empty - unexpected closer
            self.errors.push(AstError::UnbalancedStructure {
                opening_tag: name.to_string(),
                expected_closing: String::new(),
                opening_span: span,
                closing_span: None,
            });
        } else {
            // Find the matching opener
            let expected_opener = self.tag_specs.find_opener_for_closer(name);
            if let Some(opener_name) = expected_opener {
                // Search for matching opener
                let mut found_index = None;
                for (i, tag) in self.stack.iter().enumerate().rev() {
                    if tag.name == opener_name {
                        found_index = Some(i);
                        break;
                    }
                }

                if let Some(index) = found_index {
                    // Mark everything after the opener as unclosed
                    while self.stack.len() > index + 1 {
                        if let Some(unclosed) = self.stack.pop() {
                            self.errors.push(AstError::UnclosedTag {
                                tag: unclosed.name,
                                span: unclosed.span,
                            });
                        }
                    }
                    // Pop the matching opener
                    self.stack.pop();
                } else {
                    // No matching opener found
                    self.errors.push(AstError::UnbalancedStructure {
                        opening_tag: opener_name,
                        expected_closing: name.to_string(),
                        opening_span: span,
                        closing_span: None,
                    });
                }
            } else {
                // Unknown closer
                self.errors.push(AstError::UnbalancedStructure {
                    opening_tag: name.to_string(),
                    expected_closing: String::new(),
                    opening_span: span,
                    closing_span: None,
                });
            }
        }
    }

    fn current_node(&self) -> Option<&Node> {
        self.ast.nodelist().get(self.current)
    }

    fn advance(&mut self) {
        self.current += 1;
    }

    fn is_at_end(&self) -> bool {
        self.current >= self.ast.nodelist().len()
    }
}

#[must_use]
pub fn validate_template(ast: Ast, tag_specs: Arc<TagSpecs>) -> Vec<AstError> {
    TagValidator::new(ast, tag_specs).validate()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::templatetags::TagSpecs;
    use crate::Lexer;
    use crate::Parser;

    fn parse_test_template(source: &str) -> Ast {
        let tokens = Lexer::new(source).tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let (ast, _) = parser.parse().unwrap();
        ast
    }

    fn load_test_tagspecs() -> Arc<TagSpecs> {
        let toml_str = include_str!("../tagspecs/django.toml");
        Arc::new(TagSpecs::from_toml(toml_str).unwrap())
    }

    #[test]
    fn test_match_simple_if_endif() {
        let source = "{% if x %}content{% endif %}";
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let errors = validate_template(ast, tag_specs);

        assert!(errors.is_empty());
    }

    #[test]
    fn test_unclosed_if() {
        let source = "{% if x %}content";
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let errors = validate_template(ast, tag_specs);

        assert_eq!(errors.len(), 1);
        match &errors[0] {
            AstError::UnclosedTag { tag, .. } => assert_eq!(tag, "if"),
            _ => panic!("Expected UnclosedTag error"),
        }
    }

    #[test]
    fn test_mismatched_tags() {
        let source = "{% if x %}content{% endfor %}";
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let errors = validate_template(ast, tag_specs);

        assert!(!errors.is_empty());
        // Should have unexpected closer for endfor and unclosed for if
    }

    #[test]
    fn test_orphaned_else() {
        let source = "{% else %}content";
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let errors = validate_template(ast, tag_specs);

        assert_eq!(errors.len(), 1);
        match &errors[0] {
            AstError::OrphanedTag { tag, .. } => assert_eq!(tag, "else"),
            _ => panic!("Expected OrphanedTag error"),
        }
    }

    #[test]
    fn test_nested_blocks() {
        let source = "{% if x %}{% for i in items %}{{ i }}{% endfor %}{% endif %}";
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let errors = validate_template(ast, tag_specs);

        assert!(errors.is_empty());
    }

    #[test]
    fn test_complex_if_elif_else() {
        let source = "{% if x %}a{% elif y %}b{% else %}c{% endif %}";
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let errors = validate_template(ast, tag_specs);

        assert!(errors.is_empty());
    }

    #[test]
    fn test_missing_required_arguments() {
        let source = "{% load %}"; // load requires at least one argument
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let errors = validate_template(ast, tag_specs);

        assert!(!errors.is_empty());
        assert!(errors
            .iter()
            .any(|e| matches!(e, AstError::MissingRequiredArguments { .. })));
    }

    #[test]
    fn test_unnamed_endblock_closes_nearest_block() {
        let source = "{% block outer %}{% if x %}{% block inner %}test{% endblock %}{% endif %}{% endblock %}";
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let errors = validate_template(ast, tag_specs);

        assert!(errors.is_empty());
    }

    #[test]
    fn test_named_endblock_matches_named_block() {
        let source = "{% block content %}{% if x %}test{% endif %}{% endblock content %}";
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let errors = validate_template(ast, tag_specs);

        assert!(errors.is_empty());
    }

    #[test]
    fn test_mismatched_block_names() {
        let source = "{% block content %}test{% endblock footer %}";
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let errors = validate_template(ast, tag_specs);

        assert!(!errors.is_empty());
        assert!(errors
            .iter()
            .any(|e| matches!(e, AstError::UnmatchedBlockName { .. })));
    }

    #[test]
    fn test_unclosed_tags_with_unnamed_endblock() {
        let source = "{% block content %}{% if x %}test{% endblock %}";
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let errors = validate_template(ast, tag_specs);

        // Should report unclosed if
        assert!(!errors.is_empty());
        assert!(errors
            .iter()
            .any(|e| matches!(e, AstError::UnclosedTag { tag, .. } if tag == "if")));
    }
}

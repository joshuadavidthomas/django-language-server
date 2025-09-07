use std::sync::Arc;

use crate::ast::AstError;
use crate::ast::Node;
use crate::ast::Span;
use crate::tagspecs::TagSpecs;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagInfo {
    pub name: String,
    pub bits: Vec<String>,
    pub span: Span,
    pub node_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagPairs {
    pub matched_pairs: Vec<(usize, usize)>,
    pub unclosed_tags: Vec<TagInfo>,
    pub unexpected_closers: Vec<TagInfo>,
    pub mismatched_pairs: Vec<(TagInfo, TagInfo)>,
    pub orphaned_intermediates: Vec<TagInfo>,
}

pub struct TagMatcher {
    stack: Vec<TagInfo>,
    pairs: Vec<(usize, usize)>,
    unclosed_tags: Vec<TagInfo>,
    unexpected_closers: Vec<TagInfo>,
    mismatched_pairs: Vec<(TagInfo, TagInfo)>,
    orphaned_intermediates: Vec<TagInfo>,
    unmatched_block_names: Vec<(String, Span)>,
    tag_specs: Arc<TagSpecs>,
}

impl TagMatcher {
    #[must_use]
    pub fn new(tag_specs: Arc<TagSpecs>) -> Self {
        Self {
            stack: Vec::new(),
            pairs: Vec::new(),
            unclosed_tags: Vec::new(),
            unexpected_closers: Vec::new(),
            mismatched_pairs: Vec::new(),
            orphaned_intermediates: Vec::new(),
            unmatched_block_names: Vec::new(),
            tag_specs,
        }
    }

    #[must_use]
    pub fn match_tags(nodes: &[Node], tag_specs: Arc<TagSpecs>) -> (TagPairs, Vec<AstError>) {
        let mut matcher = Self::new(tag_specs);

        for (idx, node) in nodes.iter().enumerate() {
            if let Node::Tag { name, bits, span } = node {
                matcher.process_tag(idx, name.as_str(), bits, *span);
            }
        }

        matcher.finalize()
    }

    fn process_tag(&mut self, idx: usize, name: &str, bits: &[String], span: Span) {
        if self.is_opener(name) {
            self.stack.push(TagInfo {
                name: name.to_string(),
                bits: bits.to_vec(),
                span,
                node_index: idx,
            });
        } else if self.is_closer(name) {
            self.handle_closer(idx, name, bits, span);
        } else if self.is_intermediate(name) {
            self.handle_intermediate(idx, name, span);
        }
    }

    fn is_opener(&self, name: &str) -> bool {
        self.tag_specs
            .get(name)
            .and_then(|spec| spec.end.as_ref())
            .is_some()
    }

    fn is_closer(&self, name: &str) -> bool {
        self.tag_specs.is_closer(name)
    }

    fn is_intermediate(&self, name: &str) -> bool {
        self.tag_specs.is_intermediate(name)
    }

    fn handle_closer(&mut self, idx: usize, name: &str, bits: &[String], span: Span) {
        // Special handling for endblock with optional block name
        if name == "endblock" && !bits.is_empty() {
            // endblock has a specific block name to close
            let target_block_name = &bits[0];

            // Find the matching block in the stack
            let mut found_index = None;
            for (i, tag) in self.stack.iter().enumerate().rev() {
                if tag.name == "block" && !tag.bits.is_empty() && tag.bits[0] == *target_block_name
                {
                    found_index = Some(i);
                    break;
                }
            }

            if let Some(stack_index) = found_index {
                // Mark any blocks after the target as unclosed
                while self.stack.len() > stack_index + 1 {
                    if let Some(unclosed) = self.stack.pop() {
                        self.unclosed_tags.push(unclosed);
                    }
                }
                // Now pop and match the target block
                if let Some(opener) = self.stack.pop() {
                    self.pairs.push((opener.node_index, idx));
                }
            } else {
                // No matching block found - track this separately for a better error message
                self.unmatched_block_names
                    .push((target_block_name.clone(), span));
            }
        } else {
            // Normal closer handling
            if let Some(opener) = self.stack.pop() {
                let expected_closer = self.expected_closer(&opener.name);

                if expected_closer.as_deref() == Some(name) {
                    self.pairs.push((opener.node_index, idx));
                } else {
                    self.mismatched_pairs.push((
                        opener,
                        TagInfo {
                            name: name.to_string(),
                            bits: bits.to_vec(),
                            span,
                            node_index: idx,
                        },
                    ));
                }
            } else {
                self.unexpected_closers.push(TagInfo {
                    name: name.to_string(),
                    bits: bits.to_vec(),
                    span,
                    node_index: idx,
                });
            }
        }
    }

    fn handle_intermediate(&mut self, idx: usize, name: &str, span: Span) {
        // Check if this intermediate has a valid context on the stack
        let has_valid_context = if self.stack.is_empty() {
            false
        } else {
            let last = self.stack.last().unwrap();
            match name {
                "elif" | "elseif" | "else" if last.name == "if" => true,
                "empty" if last.name == "for" => true,
                _ => false,
            }
        };

        if !has_valid_context {
            // Intermediate tag without proper context is orphaned
            self.orphaned_intermediates.push(TagInfo {
                name: name.to_string(),
                bits: Vec::new(),
                span,
                node_index: idx,
            });
        }
    }

    fn expected_closer(&self, opener: &str) -> Option<String> {
        self.tag_specs
            .get(opener)
            .and_then(|spec| spec.end.as_ref())
            .map(|end| end.tag.clone())
    }

    fn finalize(mut self) -> (TagPairs, Vec<AstError>) {
        // Any remaining stack items are unclosed
        while let Some(tag) = self.stack.pop() {
            self.unclosed_tags.push(tag);
        }

        let mut errors = Vec::new();

        // Convert unclosed tags to errors
        for tag in &self.unclosed_tags {
            errors.push(AstError::UnclosedTag {
                tag: tag.name.clone(),
                span: tag.span,
            });
        }

        // Convert unexpected closers to errors
        for tag in &self.unexpected_closers {
            errors.push(AstError::UnbalancedStructure {
                opening_tag: String::new(),
                expected_closing: tag.name.clone(),
                opening_span: tag.span,
                closing_span: None,
            });
        }

        // Convert unmatched block names to errors
        for (name, span) in &self.unmatched_block_names {
            errors.push(AstError::UnmatchedBlockName {
                name: name.clone(),
                span: *span,
            });
        }

        // Convert mismatched pairs to errors
        for (opener, closer) in &self.mismatched_pairs {
            let expected = self.expected_closer(&opener.name).unwrap_or_default();
            errors.push(AstError::UnbalancedStructure {
                opening_tag: opener.name.clone(),
                expected_closing: expected,
                opening_span: opener.span,
                closing_span: Some(closer.span),
            });
        }

        // Convert orphaned intermediates to errors
        for tag in &self.orphaned_intermediates {
            let context = match tag.name.as_str() {
                "elif" | "elseif" | "else" => "must appear between 'if' and 'endif'".to_string(),
                "empty" => "must appear between 'for' and 'endfor'".to_string(),
                _ => "appears outside its required context".to_string(),
            };
            errors.push(AstError::OrphanedTag {
                tag: tag.name.clone(),
                context,
                span: tag.span,
            });
        }

        let pairs = TagPairs {
            matched_pairs: self.pairs,
            unclosed_tags: self.unclosed_tags,
            unexpected_closers: self.unexpected_closers,
            mismatched_pairs: self.mismatched_pairs,
            orphaned_intermediates: self.orphaned_intermediates,
        };

        (pairs, errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_test_tagspecs() -> Arc<TagSpecs> {
        let toml_str = include_str!("../tagspecs/django.toml");
        Arc::new(TagSpecs::from_toml(toml_str).unwrap())
    }

    #[test]
    fn test_match_simple_if_endif() {
        let source = "{% if x %}content{% endif %}";
        let (ast, _) = crate::parse_template(source).unwrap();
        let tag_specs = load_test_tagspecs();

        let (pairs, errors) = TagMatcher::match_tags(ast.nodelist(), tag_specs);

        assert!(errors.is_empty());
        assert_eq!(pairs.matched_pairs.len(), 1);
        assert!(pairs.unclosed_tags.is_empty());
        assert!(pairs.unexpected_closers.is_empty());
    }

    #[test]
    fn test_unclosed_if() {
        let source = "{% if x %}content";
        let (ast, _) = crate::parse_template(source).unwrap();
        let tag_specs = load_test_tagspecs();

        let (pairs, errors) = TagMatcher::match_tags(ast.nodelist(), tag_specs);

        assert_eq!(errors.len(), 1);
        assert!(matches!(errors[0], AstError::UnclosedTag { .. }));
        assert_eq!(pairs.unclosed_tags.len(), 1);
    }

    #[test]
    fn test_unexpected_endif() {
        let source = "content{% endif %}";
        let (ast, _) = crate::parse_template(source).unwrap();
        let tag_specs = load_test_tagspecs();

        let (pairs, errors) = TagMatcher::match_tags(ast.nodelist(), tag_specs);

        assert_eq!(errors.len(), 1);
        assert!(matches!(errors[0], AstError::UnbalancedStructure { .. }));
        assert_eq!(pairs.unexpected_closers.len(), 1);
    }

    #[test]
    fn test_mismatched_tags() {
        let source = "{% if x %}{% endfor %}";
        let (ast, _) = crate::parse_template(source).unwrap();
        let tag_specs = load_test_tagspecs();

        let (pairs, errors) = TagMatcher::match_tags(ast.nodelist(), tag_specs);

        assert_eq!(errors.len(), 1);
        assert!(matches!(errors[0], AstError::UnbalancedStructure { .. }));
        assert_eq!(pairs.mismatched_pairs.len(), 1);
    }

    #[test]
    fn test_nested_blocks() {
        let source = "{% if x %}{% for item in items %}{{ item }}{% endfor %}{% endif %}";
        let (ast, _) = crate::parse_template(source).unwrap();
        let tag_specs = load_test_tagspecs();

        let (pairs, errors) = TagMatcher::match_tags(ast.nodelist(), tag_specs);

        assert!(errors.is_empty());
        assert_eq!(pairs.matched_pairs.len(), 2);
    }

    #[test]
    fn test_if_elif_else_endif() {
        let source = "{% if x %}a{% elif y %}b{% else %}c{% endif %}";
        let (ast, _) = crate::parse_template(source).unwrap();
        let tag_specs = load_test_tagspecs();

        let (pairs, errors) = TagMatcher::match_tags(ast.nodelist(), tag_specs);

        assert!(errors.is_empty());
        assert_eq!(pairs.matched_pairs.len(), 1);
    }

    #[test]
    fn test_nested_blocks_with_named_endblock() {
        // Test case: inner block is unclosed, outer block is closed with its name
        let source = "{% block content %}{% block test %}{% endblock content %}";
        let (ast, _) = crate::parse_template(source).unwrap();
        let tag_specs = load_test_tagspecs();

        let (pairs, errors) = TagMatcher::match_tags(ast.nodelist(), tag_specs);

        // Should have one error for the unclosed inner block
        assert_eq!(errors.len(), 1);
        assert!(matches!(&errors[0], AstError::UnclosedTag { ref tag, .. } if tag == "block"));

        // The outer block should be matched correctly
        assert_eq!(pairs.matched_pairs.len(), 1);
        assert_eq!(pairs.unclosed_tags.len(), 1);
        assert_eq!(pairs.unclosed_tags[0].name, "block");
        assert_eq!(pairs.unclosed_tags[0].bits, vec!["test"]);
    }

    #[test]
    fn test_tagspec_driven_closers() {
        // Test that is_closer properly uses tagspecs
        let tag_specs = load_test_tagspecs();

        // These should be recognized as closers based on tagspecs
        assert!(tag_specs.is_closer("endif"));
        assert!(tag_specs.is_closer("endfor"));
        assert!(tag_specs.is_closer("endblock"));
        assert!(tag_specs.is_closer("endwith"));

        // These should NOT be recognized as closers
        assert!(!tag_specs.is_closer("if"));
        assert!(!tag_specs.is_closer("for"));
        assert!(!tag_specs.is_closer("block"));
        assert!(!tag_specs.is_closer("random_tag"));

        // Even tags starting with "end" shouldn't be closers unless in tagspecs
        assert!(!tag_specs.is_closer("endnotreal"));
    }

    #[test]
    fn test_tagspec_driven_intermediates() {
        // Test that is_intermediate properly uses tagspecs
        let tag_specs = load_test_tagspecs();

        // These should be recognized as intermediates based on tagspecs
        assert!(tag_specs.is_intermediate("elif"));
        assert!(tag_specs.is_intermediate("else"));
        assert!(tag_specs.is_intermediate("empty"));

        // These should NOT be recognized as intermediates
        assert!(!tag_specs.is_intermediate("if"));
        assert!(!tag_specs.is_intermediate("endif"));
        assert!(!tag_specs.is_intermediate("random"));
    }

    #[test]
    fn test_unmatched_endblock_name() {
        // Test case: endblock with a name that doesn't match any open block
        let source = "{% block test %}content{% endblock wrong %}";
        let (ast, _) = crate::parse_template(source).unwrap();
        let tag_specs = load_test_tagspecs();

        let (pairs, errors) = TagMatcher::match_tags(ast.nodelist(), tag_specs);

        // Should have two errors: unmatched block name and unclosed block
        assert_eq!(errors.len(), 2);

        // Check we have the unmatched block name error
        let has_unmatched = errors
            .iter()
            .any(|e| matches!(e, AstError::UnmatchedBlockName { ref name, .. } if name == "wrong"));
        assert!(has_unmatched, "Should have UnmatchedBlockName error");

        // Check we have the unclosed tag error
        let has_unclosed = errors
            .iter()
            .any(|e| matches!(e, AstError::UnclosedTag { ref tag, .. } if tag == "block"));
        assert!(has_unclosed, "Should have UnclosedTag error");

        // The block 'test' should still be unclosed
        assert_eq!(pairs.unclosed_tags.len(), 1);
        assert_eq!(pairs.unclosed_tags[0].name, "block");
        assert_eq!(pairs.unclosed_tags[0].bits, vec!["test"]);
    }

    #[test]
    fn test_complex_validation_errors() {
        // Test the exact case from the user - multiple validation issues
        let source = r"
    {% block test %}
      {% if test %}{% endif %}
    {% else %}
    {% block foobar %}
    {% endblock fsdfsa %}
    {% endblock test %}
    ";
        let (ast, _) = crate::parse_template(source).unwrap();
        let tag_specs = load_test_tagspecs();

        let (_pairs, errors) = TagMatcher::match_tags(ast.nodelist(), tag_specs);

        // Should have 3 errors:
        // 1. Orphaned else tag (outside if context)
        // 2. Unclosed block foobar
        // 3. Unmatched endblock fsdfsa
        assert_eq!(errors.len(), 3);

        // Check for orphaned else
        let has_orphaned = errors
            .iter()
            .any(|e| matches!(e, AstError::OrphanedTag { ref tag, .. } if tag == "else"));
        assert!(has_orphaned, "Should have OrphanedTag error for else");

        // Check for unclosed block
        let has_unclosed = errors
            .iter()
            .any(|e| matches!(e, AstError::UnclosedTag { ref tag, .. } if tag == "block"));
        assert!(has_unclosed, "Should have UnclosedTag error for block");

        // Check for unmatched endblock name
        let has_unmatched = errors.iter().any(
            |e| matches!(e, AstError::UnmatchedBlockName { ref name, .. } if name == "fsdfsa"),
        );
        assert!(
            has_unmatched,
            "Should have UnmatchedBlockName error for fsdfsa"
        );
    }

    #[test]
    fn test_else_after_endif() {
        // Test case: else appears after endif (outside of if block)
        let source = "{% if test %}content{% endif %}{% else %}other";
        let (ast, _) = crate::parse_template(source).unwrap();
        let tag_specs = load_test_tagspecs();

        let (pairs, errors) = TagMatcher::match_tags(ast.nodelist(), tag_specs);

        // Should have one error for the misplaced else
        assert_eq!(errors.len(), 1);
        assert!(matches!(&errors[0], AstError::OrphanedTag { ref tag, .. } if tag == "else"));

        // The if-endif should be matched correctly
        assert_eq!(pairs.matched_pairs.len(), 1);
        // The else should be in orphaned_intermediates
        assert_eq!(pairs.orphaned_intermediates.len(), 1);
        assert_eq!(pairs.orphaned_intermediates[0].name, "else");
    }

    #[test]
    fn test_elif_after_endif() {
        // Test case: elif appears after endif (outside of if block)
        let source = "{% if test %}content{% endif %}{% elif other %}other";
        let (ast, _) = crate::parse_template(source).unwrap();
        let tag_specs = load_test_tagspecs();

        let (pairs, errors) = TagMatcher::match_tags(ast.nodelist(), tag_specs);

        // Should have one error for the misplaced elif
        assert_eq!(errors.len(), 1);
        assert!(matches!(&errors[0], AstError::OrphanedTag { ref tag, .. } if tag == "elif"));

        // The if-endif should be matched correctly
        assert_eq!(pairs.matched_pairs.len(), 1);
        // The elif should be in orphaned_intermediates
        assert_eq!(pairs.orphaned_intermediates.len(), 1);
        assert_eq!(pairs.orphaned_intermediates[0].name, "elif");
    }

    #[test]
    fn test_else_in_wrong_context() {
        // Test case: else appears inside a for block (wrong context)
        let source = "{% for item in items %}{% else %}{% endfor %}";
        let (ast, _) = crate::parse_template(source).unwrap();
        let tag_specs = load_test_tagspecs();

        let (pairs, errors) = TagMatcher::match_tags(ast.nodelist(), tag_specs);

        // Should have one error for else in wrong context
        assert_eq!(errors.len(), 1);
        assert!(matches!(&errors[0], AstError::OrphanedTag { ref tag, .. } if tag == "else"));

        // The for should still be matched with endfor
        assert_eq!(pairs.matched_pairs.len(), 1);
        // The else should be in orphaned_intermediates
        assert_eq!(pairs.orphaned_intermediates.len(), 1);
        assert_eq!(pairs.orphaned_intermediates[0].name, "else");
    }

    #[test]
    fn test_empty_outside_for() {
        // Test case: empty appears outside of for block
        let source = "{% if test %}content{% endif %}{% empty %}";
        let (ast, _) = crate::parse_template(source).unwrap();
        let tag_specs = load_test_tagspecs();

        let (pairs, errors) = TagMatcher::match_tags(ast.nodelist(), tag_specs);

        // Should have one error for the misplaced empty
        assert_eq!(errors.len(), 1);
        assert!(matches!(&errors[0], AstError::OrphanedTag { ref tag, .. } if tag == "empty"));

        // The if-endif should be matched correctly
        assert_eq!(pairs.matched_pairs.len(), 1);
        // The empty should be in orphaned_intermediates
        assert_eq!(pairs.orphaned_intermediates.len(), 1);
        assert_eq!(pairs.orphaned_intermediates[0].name, "empty");
    }

    #[test]
    fn test_else_after_endif_inside_block() {
        // Test case: else appears after endif but inside a block
        // This was the user's specific example that was showing wrong error location
        let source = "{% block test %}{% if test %}{% endif %}{% else %}{% endblock %}";
        let (ast, _) = crate::parse_template(source).unwrap();
        let tag_specs = load_test_tagspecs();

        let (pairs, errors) = TagMatcher::match_tags(ast.nodelist(), tag_specs);

        // Should have one error for the misplaced else
        assert_eq!(errors.len(), 1, "Expected exactly one error");
        assert!(
            matches!(&errors[0], AstError::OrphanedTag { ref tag, ref context, .. }
            if tag == "else" && context.contains("if"))
        );

        // The block and if should be matched correctly
        assert_eq!(
            pairs.matched_pairs.len(),
            2,
            "Expected both block and if to be matched"
        );

        // The else should be in orphaned_intermediates with the correct span
        assert_eq!(pairs.orphaned_intermediates.len(), 1);
        assert_eq!(pairs.orphaned_intermediates[0].name, "else");

        // Ensure no unclosed tags reported
        assert_eq!(
            pairs.unclosed_tags.len(),
            0,
            "Should not report any unclosed tags"
        );
    }

    #[test]
    fn test_for_empty_endfor() {
        let source = "{% for item in items %}{{ item }}{% empty %}none{% endfor %}";
        let (ast, _) = crate::parse_template(source).unwrap();
        let tag_specs = load_test_tagspecs();

        let (pairs, errors) = TagMatcher::match_tags(ast.nodelist(), tag_specs);

        assert!(errors.is_empty());
        assert_eq!(pairs.matched_pairs.len(), 1);
    }
}

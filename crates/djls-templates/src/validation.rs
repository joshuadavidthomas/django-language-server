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
//! The validation function traverses the AST node list and maintains a stack
//! of open tags to track block nesting and validate proper closure.
//!
//! ## Adding New Validation Rules
//!
//! 1. Add the error variant to [`AstError`]
//! 2. Add tracking logic in the validation function if needed
//! 3. Add test cases in the test module
//!
//! ## Example
//!
//! ```ignore
//! use djls_templates::validation::validate_template;
//! use djls_templates::tagspecs::TagSpecs;
//!
//! let tag_specs = Arc::new(TagSpecs::default());
//! let (pairs, errors) = validate_template(&ast, tag_specs);
//! ```

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

/// Validates Django template tags and returns tag pairing information and errors.
#[must_use]
pub fn validate_template(nodes: &[Node], tag_specs: Arc<TagSpecs>) -> (TagPairs, Vec<AstError>) {
    let mut stack: Vec<TagInfo> = Vec::new();
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    let mut unclosed_tags: Vec<TagInfo> = Vec::new();
    let mut unexpected_closers: Vec<TagInfo> = Vec::new();
    let mut mismatched_pairs: Vec<(TagInfo, TagInfo)> = Vec::new();
    let mut orphaned_intermediates: Vec<TagInfo> = Vec::new();
    let mut unmatched_block_names: Vec<(String, Span)> = Vec::new();
    let mut missing_arguments: Vec<TagInfo> = Vec::new();
    let mut too_many_arguments: Vec<TagInfo> = Vec::new();
    
    let mut current_index = 0;

    for node in nodes {
        if let Node::Tag { name, bits, span } = node {
            process_tag(
                name,
                bits,
                *span,
                current_index,
                &mut stack,
                &mut pairs,
                &mut unclosed_tags,
                &mut unexpected_closers,
                &mut mismatched_pairs,
                &mut orphaned_intermediates,
                &mut unmatched_block_names,
                &mut missing_arguments,
                &mut too_many_arguments,
                &tag_specs,
            );
        }
        current_index += 1;
    }

    // Any remaining stack items are unclosed
    while let Some(tag) = stack.pop() {
        unclosed_tags.push(tag);
    }

    // Convert to errors
    let mut errors = Vec::new();

    // Convert unclosed tags to errors
    for tag in &unclosed_tags {
        errors.push(AstError::UnclosedTag {
            tag: tag.name.clone(),
            span: tag.span,
        });
    }

    // Convert unexpected closers to errors
    for tag in &unexpected_closers {
        errors.push(AstError::UnbalancedStructure {
            opening_tag: String::new(),
            expected_closing: tag.name.clone(),
            opening_span: tag.span,
            closing_span: None,
        });
    }

    // Convert unmatched block names to errors
    for (name, span) in &unmatched_block_names {
        errors.push(AstError::UnmatchedBlockName {
            name: name.clone(),
            span: *span,
        });
    }

    // Convert mismatched pairs to errors
    for (opener, closer) in &mismatched_pairs {
        let expected = expected_closer(&opener.name, &tag_specs).unwrap_or_default();
        errors.push(AstError::UnbalancedStructure {
            opening_tag: opener.name.clone(),
            expected_closing: expected,
            opening_span: opener.span,
            closing_span: Some(closer.span),
        });
    }

    // Convert orphaned intermediates to errors
    for tag in &orphaned_intermediates {
        // Find which opener(s) this intermediate belongs to using TagSpecs
        let parent_tags = tag_specs.get_parent_tags_for_intermediate(&tag.name);
        
        let context = if parent_tags.is_empty() {
            "appears outside its required context".to_string()
        } else if parent_tags.len() == 1 {
            let parent = &parent_tags[0];
            if let Some(spec) = tag_specs.get(parent) {
                if let Some(end_tag) = &spec.end {
                    format!("must appear between '{}' and '{}'", parent, end_tag.tag)
                } else {
                    format!("must appear within '{parent}' block")
                }
            } else {
                "appears outside its required context".to_string()
            }
        } else {
            // Multiple possible parent tags
            let parents = parent_tags.join("' or '");
            format!("must appear within '{parents}' block")
        };
        
        errors.push(AstError::OrphanedTag {
            tag: tag.name.clone(),
            context,
            span: tag.span,
        });
    }

    // Convert missing arguments to errors
    for tag in &missing_arguments {
        if let Some(spec) = tag_specs.get(&tag.name) {
            if let Some(arg_spec) = &spec.args {
                if let Some(min) = arg_spec.min {
                    errors.push(AstError::MissingRequiredArguments {
                        tag: tag.name.clone(),
                        min,
                        span: tag.span,
                    });
                }
            }
        }
    }

    // Convert too many arguments to errors
    for tag in &too_many_arguments {
        if let Some(spec) = tag_specs.get(&tag.name) {
            if let Some(arg_spec) = &spec.args {
                if let Some(max) = arg_spec.max {
                    errors.push(AstError::TooManyArguments {
                        tag: tag.name.clone(),
                        max,
                        span: tag.span,
                    });
                }
            }
        }
    }

    let tag_pairs = TagPairs {
        matched_pairs: pairs,
        unclosed_tags,
        unexpected_closers,
        mismatched_pairs,
        orphaned_intermediates,
    };

    (tag_pairs, errors)
}

#[allow(clippy::too_many_arguments)]
fn process_tag(
    name: &str,
    bits: &[String],
    span: Span,
    idx: usize,
    stack: &mut Vec<TagInfo>,
    pairs: &mut Vec<(usize, usize)>,
    unclosed_tags: &mut Vec<TagInfo>,
    unexpected_closers: &mut Vec<TagInfo>,
    mismatched_pairs: &mut Vec<(TagInfo, TagInfo)>,
    orphaned_intermediates: &mut Vec<TagInfo>,
    unmatched_block_names: &mut Vec<(String, Span)>,
    missing_arguments: &mut Vec<TagInfo>,
    too_many_arguments: &mut Vec<TagInfo>,
    tag_specs: &Arc<TagSpecs>,
) {
    // Check argument requirements first
    if let Some(spec) = tag_specs.get(name) {
        if let Some(arg_spec) = &spec.args {
            if let Some(min) = arg_spec.min {
                if bits.len() < min {
                    missing_arguments.push(TagInfo {
                        name: name.to_string(),
                        bits: bits.to_vec(),
                        span,
                        node_index: idx,
                    });
                }
            }
            if let Some(max) = arg_spec.max {
                if bits.len() > max {
                    too_many_arguments.push(TagInfo {
                        name: name.to_string(),
                        bits: bits.to_vec(),
                        span,
                        node_index: idx,
                    });
                }
            }
        }
    }

    if is_opener(name, tag_specs) {
        stack.push(TagInfo {
            name: name.to_string(),
            bits: bits.to_vec(),
            span,
            node_index: idx,
        });
    } else if is_closer(name, tag_specs) {
        handle_closer(
            name,
            bits,
            span,
            idx,
            stack,
            pairs,
            unclosed_tags,
            unexpected_closers,
            mismatched_pairs,
            unmatched_block_names,
            tag_specs,
        );
    } else if is_intermediate(name, tag_specs) {
        handle_intermediate(name, span, idx, stack, orphaned_intermediates, tag_specs);
    }
}

fn is_opener(name: &str, tag_specs: &Arc<TagSpecs>) -> bool {
    tag_specs
        .get(name)
        .and_then(|spec| spec.end.as_ref())
        .is_some()
}

fn is_closer(name: &str, tag_specs: &Arc<TagSpecs>) -> bool {
    tag_specs.is_closer(name)
}

fn is_intermediate(name: &str, tag_specs: &Arc<TagSpecs>) -> bool {
    tag_specs.is_intermediate(name)
}

#[allow(clippy::too_many_arguments)]
fn handle_closer(
    name: &str,
    bits: &[String],
    span: Span,
    idx: usize,
    stack: &mut Vec<TagInfo>,
    pairs: &mut Vec<(usize, usize)>,
    unclosed_tags: &mut Vec<TagInfo>,
    unexpected_closers: &mut Vec<TagInfo>,
    mismatched_pairs: &mut Vec<(TagInfo, TagInfo)>,
    unmatched_block_names: &mut Vec<(String, Span)>,
    tag_specs: &Arc<TagSpecs>,
) {
    // Special handling for endblock
    if name == "endblock" {
        if !bits.is_empty() {
            // endblock has a specific block name to close
            let target_block_name = &bits[0];

            // Find the matching block in the stack
            let mut found_index = None;
            for (i, tag) in stack.iter().enumerate().rev() {
                if tag.name == "block" && !tag.bits.is_empty() && tag.bits[0] == *target_block_name
                {
                    found_index = Some(i);
                    break;
                }
            }

            if let Some(stack_index) = found_index {
                // Mark any blocks after the target as unclosed
                while stack.len() > stack_index + 1 {
                    if let Some(unclosed) = stack.pop() {
                        unclosed_tags.push(unclosed);
                    }
                }
                // Now pop and match the target block
                if let Some(opener) = stack.pop() {
                    pairs.push((opener.node_index, idx));
                }
            } else {
                // No matching block found - track this separately for a better error message
                unmatched_block_names.push((target_block_name.clone(), span));
            }
        } else {
            // Unnamed endblock - find the nearest block on the stack
            let mut found_index = None;
            for (i, tag) in stack.iter().enumerate().rev() {
                if tag.name == "block" {
                    found_index = Some(i);
                    break;
                }
            }

            if let Some(stack_index) = found_index {
                // Mark any tags after the block as unclosed
                while stack.len() > stack_index + 1 {
                    if let Some(unclosed) = stack.pop() {
                        unclosed_tags.push(unclosed);
                    }
                }
                // Now pop and match the block
                if let Some(opener) = stack.pop() {
                    pairs.push((opener.node_index, idx));
                }
            } else {
                // No block found on stack - treat as unexpected closer
                unexpected_closers.push(TagInfo {
                    name: name.to_string(),
                    bits: bits.to_vec(),
                    span,
                    node_index: idx,
                });
            }
        }
    } else {
        // Normal closer handling for non-endblock tags
        if let Some(opener) = stack.pop() {
            let expected = expected_closer(&opener.name, tag_specs);

            if expected.as_deref() == Some(name) {
                pairs.push((opener.node_index, idx));
            } else {
                mismatched_pairs.push((
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
            unexpected_closers.push(TagInfo {
                name: name.to_string(),
                bits: bits.to_vec(),
                span,
                node_index: idx,
            });
        }
    }
}

fn handle_intermediate(
    name: &str,
    span: Span,
    idx: usize,
    stack: &[TagInfo],
    orphaned_intermediates: &mut Vec<TagInfo>,
    tag_specs: &Arc<TagSpecs>,
) {
    // Check if this intermediate has a valid context on the stack using TagSpecs
    let has_valid_context = if stack.is_empty() {
        false
    } else {
        let last = &stack[stack.len() - 1];
        // Check if the current tag on stack has this intermediate in its spec
        if let Some(spec) = tag_specs.get(&last.name) {
            spec.intermediates
                .as_ref()
                .is_some_and(|intermediates| intermediates.contains(&name.to_string()))
        } else {
            false
        }
    };

    if !has_valid_context {
        // Intermediate tag without proper context is orphaned
        orphaned_intermediates.push(TagInfo {
            name: name.to_string(),
            bits: Vec::new(),
            span,
            node_index: idx,
        });
    }
}

fn expected_closer(opener: &str, tag_specs: &Arc<TagSpecs>) -> Option<String> {
    tag_specs
        .get(opener)
        .and_then(|spec| spec.end.as_ref())
        .map(|end| end.tag.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Ast, Lexer, Parser};
    use crate::tagspecs::TagSpecs;
    use std::sync::Arc;

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

        let (pairs, errors) = validate_template(ast.nodelist(), tag_specs);

        assert!(errors.is_empty());
        assert_eq!(pairs.matched_pairs.len(), 1);
        assert!(pairs.unclosed_tags.is_empty());
        assert!(pairs.unexpected_closers.is_empty());
    }

    #[test]
    fn test_unclosed_if() {
        let source = "{% if x %}content";
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let (pairs, errors) = validate_template(ast.nodelist(), tag_specs);

        assert_eq!(errors.len(), 1);
        assert!(matches!(errors[0], AstError::UnclosedTag { .. }));
        assert_eq!(pairs.unclosed_tags.len(), 1);
    }

    #[test]
    fn test_unexpected_endif() {
        let source = "content{% endif %}";
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let (pairs, errors) = validate_template(ast.nodelist(), tag_specs);

        assert_eq!(errors.len(), 1);
        assert!(matches!(errors[0], AstError::UnbalancedStructure { .. }));
        assert_eq!(pairs.unexpected_closers.len(), 1);
    }

    #[test]
    fn test_mismatched_tags() {
        let source = "{% if x %}{% endfor %}";
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let (pairs, errors) = validate_template(ast.nodelist(), tag_specs);

        assert_eq!(errors.len(), 1);
        assert!(matches!(errors[0], AstError::UnbalancedStructure { .. }));
        assert_eq!(pairs.mismatched_pairs.len(), 1);
    }

    #[test]
    fn test_nested_blocks() {
        let source = "{% if x %}{% for item in items %}{{ item }}{% endfor %}{% endif %}";
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let (pairs, errors) = validate_template(ast.nodelist(), tag_specs);

        assert!(errors.is_empty());
        assert_eq!(pairs.matched_pairs.len(), 2);
    }

    #[test]
    fn test_if_elif_else_endif() {
        let source = "{% if x %}a{% elif y %}b{% else %}c{% endif %}";
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let (pairs, errors) = validate_template(ast.nodelist(), tag_specs);

        assert!(errors.is_empty());
        assert_eq!(pairs.matched_pairs.len(), 1);
    }

    #[test]
    fn test_nested_blocks_with_named_endblock() {
        // Test case: inner block is unclosed, outer block is closed with its name
        let source = "{% block content %}{% block test %}{% endblock content %}";
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let (pairs, errors) = validate_template(ast.nodelist(), tag_specs);

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
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let (pairs, errors) = validate_template(ast.nodelist(), tag_specs);

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
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let (_pairs, errors) = validate_template(ast.nodelist(), tag_specs);

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
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let (pairs, errors) = validate_template(ast.nodelist(), tag_specs);

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
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let (pairs, errors) = validate_template(ast.nodelist(), tag_specs);

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
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let (pairs, errors) = validate_template(ast.nodelist(), tag_specs);

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
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let (pairs, errors) = validate_template(ast.nodelist(), tag_specs);

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
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let (pairs, errors) = validate_template(ast.nodelist(), tag_specs);

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
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let (pairs, errors) = validate_template(ast.nodelist(), tag_specs);

        assert!(errors.is_empty());
        assert_eq!(pairs.matched_pairs.len(), 1);
    }

    #[test]
    fn test_block_missing_arguments() {
        let source = "{% block %}content{% endblock %}";
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let (_, errors) = validate_template(ast.nodelist(), tag_specs);

        // Should have error for missing block name
        assert_eq!(errors.len(), 1, "Expected exactly one error");
        assert!(
            matches!(
                &errors[0],
                AstError::MissingRequiredArguments { tag, min, .. }
                if tag == "block" && *min == 1
            ),
            "Error should be MissingRequiredArguments for block"
        );
    }

    #[test]
    fn test_extends_missing_arguments() {
        let source = "{% extends %}";
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let (_, errors) = validate_template(ast.nodelist(), tag_specs);

        // Should have error for missing template name
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            &errors[0],
            AstError::MissingRequiredArguments { tag, min, .. }
            if tag == "extends" && *min == 1
        ));
    }

    #[test]
    fn test_csrf_token_with_arguments() {
        let source = "{% csrf_token some_arg %}";
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let (_, errors) = validate_template(ast.nodelist(), tag_specs);

        // Should have error for too many arguments (csrf_token takes none)
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            &errors[0],
            AstError::TooManyArguments { tag, max, .. }
            if tag == "csrf_token" && *max == 0
        ));
    }

    #[test]
    fn test_block_too_many_arguments() {
        let source = "{% block content extra %}content{% endblock %}";
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let (_, errors) = validate_template(ast.nodelist(), tag_specs);

        // Should have error for too many arguments
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            &errors[0],
            crate::ast::AstError::TooManyArguments { tag, max, .. }
            if tag == "block" && *max == 1
        ));
    }

    #[test]
    fn test_load_missing_arguments() {
        let source = "{% load %}";
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let (_, errors) = validate_template(ast.nodelist(), tag_specs);

        // Should have error for missing library name
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            &errors[0],
            crate::ast::AstError::MissingRequiredArguments { tag, min, .. }
            if tag == "load" && *min == 1
        ));
    }

    #[test]
    fn test_unnamed_endblock_with_nested_unclosed_if() {
        // Test case from issue: unnamed endblock should close the nearest block,
        // not whatever is on top of the stack
        let source = r#"{% block content %}
  {% block foo %}
    {% if foo %}
  {% endblock %}
{% endblock content %}"#;
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let (pairs, errors) = validate_template(ast.nodelist(), tag_specs);

        // Should have exactly one error: unclosed if
        assert_eq!(errors.len(), 1, "Expected exactly one error for unclosed if");
        assert!(
            matches!(&errors[0], AstError::UnclosedTag { ref tag, .. } if tag == "if"),
            "Error should be for unclosed 'if' tag"
        );

        // Both blocks should be matched correctly
        assert_eq!(pairs.matched_pairs.len(), 2, "Both block tags should be matched");
        
        // The if should be in unclosed_tags
        assert_eq!(pairs.unclosed_tags.len(), 1);
        assert_eq!(pairs.unclosed_tags[0].name, "if");
    }

    #[test]
    fn test_unnamed_endblock_closes_correct_block() {
        // Test that unnamed endblock closes the right block even with nested structures
        let source = r#"{% block outer %}
  {% if condition %}
    {% block inner %}
      content
    {% endblock %}
  {% endif %}
{% endblock %}"#;
        let ast = parse_test_template(source);
        let tag_specs = load_test_tagspecs();

        let (pairs, errors) = validate_template(ast.nodelist(), tag_specs);

        // Should have no errors
        assert!(errors.is_empty(), "Should have no validation errors");

        // All tags should be matched correctly
        assert_eq!(pairs.matched_pairs.len(), 3, "All three pairs should be matched");
        assert!(pairs.unclosed_tags.is_empty());
        assert!(pairs.unexpected_closers.is_empty());
        assert!(pairs.mismatched_pairs.is_empty());
    }
}
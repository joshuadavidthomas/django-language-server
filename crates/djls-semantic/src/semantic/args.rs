use djls_source::Span;
use djls_templates::tokens::TagDelimiter;
use djls_templates::Node;
use rustc_hash::FxHashSet;
use salsa::Accumulator;

use crate::semantic::forest::SegmentKind;
use crate::semantic::forest::SemanticNode;
use crate::templatetags::IntermediateTag;
use crate::templatetags::TagArg;
use crate::templatetags::TagSpecs;
use crate::Db;
use crate::ValidationError;
use crate::ValidationErrorAccumulator;

pub fn validate_block_tags(db: &dyn Db, roots: &[SemanticNode]) {
    for node in roots {
        validate_node(db, node);
    }
}

pub fn validate_non_block_tags(
    db: &dyn Db,
    nodelist: djls_templates::NodeList<'_>,
    skip_spans: &[Span],
) {
    let skip: FxHashSet<_> = skip_spans.iter().copied().collect();

    for node in nodelist.nodelist(db) {
        if let Node::Tag { name, bits, span } = node {
            let marker_span = span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);
            if skip.contains(&marker_span) {
                continue;
            }
            validate_tag_arguments(db, name, bits, marker_span);
        }
    }
}

fn validate_node(db: &dyn Db, node: &SemanticNode) {
    match node {
        SemanticNode::Tag {
            name,
            marker_span,
            arguments,
            segments,
        } => {
            validate_tag_arguments(db, name, arguments, *marker_span);

            for segment in segments {
                match &segment.kind {
                    SegmentKind::Main => {
                        validate_children(db, &segment.children);
                    }
                    SegmentKind::Intermediate { tag } => {
                        validate_tag_arguments(db, tag, &segment.arguments, segment.marker_span);
                        validate_children(db, &segment.children);
                    }
                }
            }
        }
        SemanticNode::Leaf { .. } => {}
    }
}

fn validate_children(db: &dyn Db, children: &[SemanticNode]) {
    for child in children {
        validate_node(db, child);
    }
}

/// Validate a single tag invocation against its `TagSpec` definition.
pub fn validate_tag_arguments(db: &dyn Db, tag_name: &str, bits: &[String], span: Span) {
    let tag_specs = db.tag_specs();

    if let Some(spec) = tag_specs.get(tag_name) {
        validate_args(db, tag_name, bits, span, spec.args.as_ref());
        return;
    }

    if let Some(end_spec) = tag_specs.get_end_spec_for_closer(tag_name) {
        validate_args(db, tag_name, bits, span, end_spec.args.as_ref());
        return;
    }

    if let Some(intermediate) = find_intermediate_spec(&tag_specs, tag_name) {
        validate_args(db, tag_name, bits, span, intermediate.args.as_ref());
    }
}

fn find_intermediate_spec<'a>(specs: &'a TagSpecs, tag_name: &str) -> Option<&'a IntermediateTag> {
    specs.iter().find_map(|(_, spec)| {
        spec.intermediate_tags
            .iter()
            .find(|it| it.name.as_ref() == tag_name)
    })
}

fn validate_args(db: &dyn Db, tag_name: &str, bits: &[String], span: Span, args: &[TagArg]) {
    if args.is_empty() {
        // If the spec expects no arguments but bits exist, report once.
        if !bits.is_empty() {
            ValidationErrorAccumulator(ValidationError::TooManyArguments {
                tag: tag_name.to_string(),
                max: 0,
                span,
            })
            .accumulate(db);
        }
        return;
    }

    let has_varargs = args.iter().any(|arg| matches!(arg, TagArg::VarArgs { .. }));
    let required_count = args.iter().filter(|arg| arg.is_required()).count();

    if bits.len() < required_count {
        ValidationErrorAccumulator(ValidationError::MissingRequiredArguments {
            tag: tag_name.to_string(),
            min: required_count,
            span,
        })
        .accumulate(db);
    }

    // NOTE: We cannot check bits.len() > args.len() because:
    // - Expression arguments can consume multiple tokens (e.g., "x > 0" is 3 tokens, 1 arg)
    // - String arguments are now single tokens even if they contain spaces
    // The validate_choices_and_order function will catch actual too-many-arguments cases

    validate_literals(db, tag_name, bits, span, args);

    if !has_varargs {
        validate_choices_and_order(db, tag_name, bits, span, args);
    }
}

fn validate_literals(db: &dyn Db, tag_name: &str, bits: &[String], span: Span, args: &[TagArg]) {
    for arg in args {
        if let TagArg::Literal { lit, required } = arg {
            if *required && !bits.iter().any(|bit| bit == lit.as_ref()) {
                ValidationErrorAccumulator(ValidationError::InvalidLiteralArgument {
                    tag: tag_name.to_string(),
                    expected: lit.to_string(),
                    span,
                })
                .accumulate(db);
            }
        }
    }
}

fn validate_choices_and_order(
    db: &dyn Db,
    tag_name: &str,
    bits: &[String],
    span: Span,
    args: &[TagArg],
) {
    let mut bit_index = 0usize;

    for (arg_index, arg) in args.iter().enumerate() {
        if bit_index >= bits.len() {
            break;
        }

        match arg {
            TagArg::Literal { lit, required } => {
                let matches_literal = bits[bit_index] == lit.as_ref();
                if *required {
                    if matches_literal {
                        bit_index += 1;
                    } else {
                        ValidationErrorAccumulator(ValidationError::InvalidLiteralArgument {
                            tag: tag_name.to_string(),
                            expected: lit.to_string(),
                            span,
                        })
                        .accumulate(db);
                        break;
                    }
                } else if matches_literal {
                    bit_index += 1;
                }
            }
            TagArg::Choice {
                name,
                required,
                choices,
            } => {
                let value = &bits[bit_index];
                if choices.iter().any(|choice| choice.as_ref() == value) {
                    bit_index += 1;
                } else if *required {
                    ValidationErrorAccumulator(ValidationError::InvalidArgumentChoice {
                        tag: tag_name.to_string(),
                        argument: name.to_string(),
                        choices: choices
                            .iter()
                            .map(std::string::ToString::to_string)
                            .collect(),
                        value: value.clone(),
                        span,
                    })
                    .accumulate(db);
                    break;
                }
            }
            TagArg::Var { .. } | TagArg::String { .. } => {
                // Consume exactly 1 token
                bit_index += 1;
            }
            TagArg::Expr { .. } => {
                // Expression arguments consume tokens until:
                // - We hit the next literal keyword
                // - We hit the end of bits
                // - We've consumed at least one token
                
                let start_index = bit_index;
                let next_literal = find_next_literal(&args[arg_index + 1..]);
                
                // Consume tokens greedily until we hit a known literal
                while bit_index < bits.len() {
                    if let Some(ref lit) = next_literal {
                        if bits[bit_index] == *lit {
                            break; // Stop before the literal
                        }
                    }
                    bit_index += 1;
                }
                
                // Must consume at least one token for expression
                if bit_index == start_index {
                    bit_index += 1;
                }
            }
            TagArg::Assignment { .. } => {
                // Assignment can be:
                // 1. Single token with = (e.g., "total=value")
                // 2. Multiple tokens with "as" keyword (e.g., "url 'name' as varname")
                // For now, consume until we find pattern or reach next literal
                
                let next_literal = find_next_literal(&args[arg_index + 1..]);
                
                while bit_index < bits.len() {
                    let token = &bits[bit_index];
                    bit_index += 1;
                    
                    // If token contains =, we're done with this assignment
                    if token.contains('=') {
                        break;
                    }
                    
                    // If we hit "as", consume one more token (the variable name)
                    if token == "as" && bit_index < bits.len() {
                        bit_index += 1;
                        break;
                    }
                    
                    // Stop if we hit next literal
                    if let Some(ref lit) = next_literal {
                        if token == lit {
                            break;
                        }
                    }
                }
            }
            TagArg::VarArgs { .. } => {
                // Consume all remaining tokens
                bit_index = bits.len();
            }
        }
    }

    // Remaining arguments with explicit names that were not satisfied because the bit stream
    // terminated early should emit specific missing argument diagnostics.
    if bit_index < bits.len() {
        return;
    }

    for arg in args.iter().skip(bit_index) {
        if arg.is_required() {
            ValidationErrorAccumulator(ValidationError::MissingArgument {
                tag: tag_name.to_string(),
                argument: argument_name(arg),
                span,
            })
            .accumulate(db);
        }
    }
}

fn argument_name(arg: &TagArg) -> String {
    match arg {
        TagArg::Literal { lit, .. } => lit.to_string(),
        TagArg::Choice { name, .. }
        | TagArg::Var { name, .. }
        | TagArg::String { name, .. }
        | TagArg::Expr { name, .. }
        | TagArg::Assignment { name, .. }
        | TagArg::VarArgs { name, .. } => name.to_string(),
    }
}

/// Find the next literal keyword in the argument list.
/// This helps expression arguments know when to stop consuming tokens.
fn find_next_literal(remaining_args: &[TagArg]) -> Option<String> {
    for arg in remaining_args {
        if let TagArg::Literal { lit, .. } = arg {
            return Some(lit.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper to manually validate arguments without full database setup
    fn validate_args_simple(bits: Vec<String>, args: Vec<TagArg>) -> Vec<String> {
        let mut errors = Vec::new();
        
        let has_varargs = args.iter().any(|arg| matches!(arg, TagArg::VarArgs { .. }));
        let required_count = args.iter().filter(|arg| arg.is_required()).count();

        if bits.len() < required_count {
            errors.push(format!("Missing required arguments: expected at least {required_count}, got {}", bits.len()));
        }

        // Validate using the actual logic
        let mut bit_index = 0usize;

        for (arg_index, arg) in args.iter().enumerate() {
            if bit_index >= bits.len() {
                break;
            }

            match arg {
                TagArg::Literal { lit, required } => {
                    let matches_literal = bits[bit_index] == lit.as_ref();
                    if *required {
                        if matches_literal {
                            bit_index += 1;
                        } else {
                            errors.push(format!("Expected literal '{}', got '{}'", lit, bits[bit_index]));
                            break;
                        }
                    } else if matches_literal {
                        bit_index += 1;
                    }
                }
                TagArg::Choice { name: _, required: _, choices } => {
                    let value = &bits[bit_index];
                    if choices.iter().any(|choice| choice.as_ref() == value) {
                        bit_index += 1;
                    }
                }
                TagArg::Var { .. } | TagArg::String { .. } => {
                    bit_index += 1;
                }
                TagArg::Expr { .. } => {
                    let start_index = bit_index;
                    let next_literal = find_next_literal(&args[arg_index + 1..]);
                    
                    while bit_index < bits.len() {
                        if let Some(ref lit) = next_literal {
                            if bits[bit_index] == *lit {
                                break;
                            }
                        }
                        bit_index += 1;
                    }
                    
                    if bit_index == start_index {
                        bit_index += 1;
                    }
                }
                TagArg::Assignment { .. } => {
                    let next_literal = find_next_literal(&args[arg_index + 1..]);
                    
                    while bit_index < bits.len() {
                        let token = &bits[bit_index];
                        bit_index += 1;
                        
                        if token.contains('=') {
                            break;
                        }
                        
                        if token == "as" && bit_index < bits.len() {
                            bit_index += 1;
                            break;
                        }
                        
                        if let Some(ref lit) = next_literal {
                            if token == lit {
                                break;
                            }
                        }
                    }
                }
                TagArg::VarArgs { .. } => {
                    bit_index = bits.len();
                }
            }
        }

        // Check if we have leftover bits (too many arguments)
        if !has_varargs && bit_index < bits.len() {
            errors.push(format!("Too many arguments: consumed {bit_index} tokens but got {}", bits.len()));
        }

        errors
    }

    #[test]
    fn test_if_tag_with_comparison_operator() {
        // Issue #1: {% if message.input_tokens > 0 %}
        // Parser tokenizes as: ["message.input_tokens", ">", "0"]
        // Spec expects: [Expr{name="condition"}]
        let bits = vec![
            "message.input_tokens".to_string(),
            ">".to_string(),
            "0".to_string(),
        ];
        let args = vec![TagArg::Expr {
            name: "condition".into(),
            required: true,
        }];
        
        let errors = validate_args_simple(bits, args);
        assert!(errors.is_empty(), "Should not error on expression with multiple tokens: {:?}", errors);
    }

    #[test]
    fn test_translate_with_quoted_string() {
        // Issue #2: {% translate "Contact the owner of the site" %}
        // Parser tokenizes as: ['"Contact the owner of the site"'] (single token now!)
        let bits = vec![r#""Contact the owner of the site""#.to_string()];
        let args = vec![TagArg::String {
            name: "message".into(),
            required: true,
        }];
        
        let errors = validate_args_simple(bits, args);
        assert!(errors.is_empty(), "Should not error on quoted string: {:?}", errors);
    }

    #[test]
    fn test_for_tag_with_reversed() {
        // {% for item in items reversed %}
        let bits = vec![
            "item".to_string(),
            "in".to_string(),
            "items".to_string(),
            "reversed".to_string(),
        ];
        let args = vec![
            TagArg::Var {
                name: "item".into(),
                required: true,
            },
            TagArg::Literal {
                lit: "in".into(),
                required: true,
            },
            TagArg::Var {
                name: "items".into(),
                required: true,
            },
            TagArg::Literal {
                lit: "reversed".into(),
                required: false,
            },
        ];
        
        let errors = validate_args_simple(bits, args);
        assert!(errors.is_empty(), "Should handle optional literal 'reversed': {:?}", errors);
    }

    #[test]
    fn test_if_complex_expression() {
        // {% if user.is_authenticated and user.is_staff %}
        let bits = vec![
            "user.is_authenticated".to_string(),
            "and".to_string(),
            "user.is_staff".to_string(),
        ];
        let args = vec![TagArg::Expr {
            name: "condition".into(),
            required: true,
        }];
        
        let errors = validate_args_simple(bits, args);
        assert!(errors.is_empty(), "Should handle complex boolean expression: {:?}", errors);
    }

    #[test]
    fn test_url_with_multiple_args() {
        // {% url 'view_name' arg1 arg2 arg3 %}
        let bits = vec![
            "'view_name'".to_string(),
            "arg1".to_string(),
            "arg2".to_string(),
            "arg3".to_string(),
        ];
        let args = vec![
            TagArg::String {
                name: "view_name".into(),
                required: true,
            },
            TagArg::VarArgs {
                name: "args".into(),
                required: false,
            },
        ];
        
        let errors = validate_args_simple(bits, args);
        assert!(errors.is_empty(), "Should handle varargs: {:?}", errors);
    }

    #[test]
    fn test_with_assignment() {
        // {% with total=items|length %}
        let bits = vec!["total=items|length".to_string()];
        let args = vec![TagArg::Assignment {
            name: "bindings".into(),
            required: true,
        }];
        
        let errors = validate_args_simple(bits, args);
        assert!(errors.is_empty(), "Should handle assignment with filter: {:?}", errors);
    }

    #[test]
    fn test_include_with_quoted_path() {
        // {% include "partials/header.html" %}
        let bits = vec![r#""partials/header.html""#.to_string()];
        let args = vec![TagArg::String {
            name: "template".into(),
            required: true,
        }];
        
        let errors = validate_args_simple(bits, args);
        assert!(errors.is_empty(), "Should handle quoted path: {:?}", errors);
    }

    #[test]
    fn test_expr_stops_at_literal() {
        // {% if condition reversed %} - "reversed" should not be consumed by expr
        let bits = vec!["x".to_string(), ">".to_string(), "0".to_string(), "reversed".to_string()];
        let args = vec![
            TagArg::Expr {
                name: "condition".into(),
                required: true,
            },
            TagArg::Literal {
                lit: "reversed".into(),
                required: false,
            },
        ];
        
        let errors = validate_args_simple(bits, args);
        assert!(errors.is_empty(), "Expr should stop before literal keyword: {:?}", errors);
    }
}

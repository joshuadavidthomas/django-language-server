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
    let specs = db.tag_specs();
    let errors = validate_block_tags_pure(&specs, roots);
    for error in errors {
        ValidationErrorAccumulator(error).accumulate(db);
    }
}

pub fn validate_non_block_tags(
    db: &dyn Db,
    nodelist: djls_templates::NodeList<'_>,
    skip_spans: &[Span],
) {
    let specs = db.tag_specs();
    let nodes = nodelist.nodelist(db);
    let errors = validate_non_block_tags_pure(&specs, nodes, skip_spans);
    for error in errors {
        ValidationErrorAccumulator(error).accumulate(db);
    }
}



fn find_intermediate_spec<'a>(specs: &'a TagSpecs, tag_name: &str) -> Option<&'a IntermediateTag> {
    specs.iter().find_map(|(_, spec)| {
        spec.intermediate_tags
            .iter()
            .find(|it| it.name.as_ref() == tag_name)
    })
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

// Pure validation functions that return errors instead of accumulating

pub fn validate_block_tags_pure(specs: &TagSpecs, roots: &[SemanticNode]) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    for node in roots {
        validate_node_pure(specs, node, &mut errors);
    }
    errors
}

pub fn validate_non_block_tags_pure(
    specs: &TagSpecs,
    nodes: &[Node],
    skip_spans: &[Span],
) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    let skip: FxHashSet<_> = skip_spans.iter().copied().collect();

    for node in nodes {
        if let Node::Tag { name, bits, span } = node {
            let marker_span = span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);
            if skip.contains(&marker_span) {
                continue;
            }
            validate_tag_arguments_pure(specs, name, bits, marker_span, &mut errors);
        }
    }
    errors
}

fn validate_node_pure(specs: &TagSpecs, node: &SemanticNode, errors: &mut Vec<ValidationError>) {
    match node {
        SemanticNode::Tag {
            name,
            marker_span,
            arguments,
            segments,
        } => {
            validate_tag_arguments_pure(specs, name, arguments, *marker_span, errors);

            for segment in segments {
                match &segment.kind {
                    SegmentKind::Main => {
                        validate_children_pure(specs, &segment.children, errors);
                    }
                    SegmentKind::Intermediate { tag } => {
                        validate_tag_arguments_pure(specs, tag, &segment.arguments, segment.marker_span, errors);
                        validate_children_pure(specs, &segment.children, errors);
                    }
                }
            }
        }
        SemanticNode::Leaf { .. } => {}
    }
}

fn validate_children_pure(specs: &TagSpecs, children: &[SemanticNode], errors: &mut Vec<ValidationError>) {
    for child in children {
        validate_node_pure(specs, child, errors);
    }
}

pub fn validate_tag_arguments_pure(
    specs: &TagSpecs,
    tag_name: &str,
    bits: &[String],
    span: Span,
    errors: &mut Vec<ValidationError>,
) {
    if let Some(spec) = specs.get(tag_name) {
        validate_args_pure(tag_name, bits, span, spec.args.as_ref(), errors);
        return;
    }

    if let Some(end_spec) = specs.get_end_spec_for_closer(tag_name) {
        validate_args_pure(tag_name, bits, span, end_spec.args.as_ref(), errors);
        return;
    }

    if let Some(intermediate) = find_intermediate_spec(specs, tag_name) {
        validate_args_pure(tag_name, bits, span, intermediate.args.as_ref(), errors);
    }
}

fn validate_args_pure(
    tag_name: &str,
    bits: &[String],
    span: Span,
    args: &[TagArg],
    errors: &mut Vec<ValidationError>,
) {
    if args.is_empty() {
        if !bits.is_empty() {
            errors.push(ValidationError::TooManyArguments {
                tag: tag_name.to_string(),
                max: 0,
                span,
            });
        }
        return;
    }

    let has_varargs = args.iter().any(|arg| matches!(arg, TagArg::VarArgs { .. }));
    let required_count = args.iter().filter(|arg| arg.is_required()).count();

    if bits.len() < required_count {
        errors.push(ValidationError::MissingRequiredArguments {
            tag: tag_name.to_string(),
            min: required_count,
            span,
        });
    }

    if !has_varargs && bits.len() > args.len() {
        errors.push(ValidationError::TooManyArguments {
            tag: tag_name.to_string(),
            max: args.len(),
            span,
        });
    }

    validate_literals_pure(tag_name, bits, span, args, errors);

    if !has_varargs {
        validate_choices_and_order_pure(tag_name, bits, span, args, errors);
    }
}

fn validate_literals_pure(
    tag_name: &str,
    bits: &[String],
    span: Span,
    args: &[TagArg],
    errors: &mut Vec<ValidationError>,
) {
    for arg in args {
        if let TagArg::Literal { lit, required } = arg {
            if *required && !bits.iter().any(|bit| bit == lit.as_ref()) {
                errors.push(ValidationError::InvalidLiteralArgument {
                    tag: tag_name.to_string(),
                    expected: lit.to_string(),
                    span,
                });
            }
        }
    }
}

fn validate_choices_and_order_pure(
    tag_name: &str,
    bits: &[String],
    span: Span,
    args: &[TagArg],
    errors: &mut Vec<ValidationError>,
) {
    let mut bit_index = 0usize;

    for arg in args {
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
                        errors.push(ValidationError::InvalidLiteralArgument {
                            tag: tag_name.to_string(),
                            expected: lit.to_string(),
                            span,
                        });
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
                    errors.push(ValidationError::InvalidArgumentChoice {
                        tag: tag_name.to_string(),
                        argument: name.to_string(),
                        choices: choices
                            .iter()
                            .map(std::string::ToString::to_string)
                            .collect(),
                        value: value.clone(),
                        span,
                    });
                    break;
                }
            }
            TagArg::Var { .. }
            | TagArg::String { .. }
            | TagArg::Expr { .. }
            | TagArg::Assignment { .. }
            | TagArg::VarArgs { .. } => {
                bit_index += 1;
            }
        }
    }

    if bit_index < bits.len() {
        return;
    }

    for arg in args.iter().skip(bit_index) {
        if arg.is_required() {
            errors.push(ValidationError::MissingArgument {
                tag: tag_name.to_string(),
                argument: argument_name(arg),
                span,
            });
        }
    }
}

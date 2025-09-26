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

    if !has_varargs && bits.len() > args.len() {
        ValidationErrorAccumulator(ValidationError::TooManyArguments {
            tag: tag_name.to_string(),
            max: args.len(),
            span,
        })
        .accumulate(db);
    }

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
            TagArg::Var { .. }
            | TagArg::String { .. }
            | TagArg::Expr { .. }
            | TagArg::Assignment { .. }
            | TagArg::VarArgs { .. } => {
                bit_index += 1;
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

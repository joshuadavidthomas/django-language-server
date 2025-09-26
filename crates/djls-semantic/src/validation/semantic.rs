use djls_source::Span;
use salsa::Accumulator;

use crate::templatetags::IntermediateTag;
use crate::templatetags::TagArg;
use crate::templatetags::TagSpecs;
use crate::Db;
use crate::ValidationError;
use crate::ValidationErrorAccumulator;

/// Validate a single tag invocation against its `TagSpec` definition.
pub fn validate_tag(db: &dyn Db, tag_name: &str, bits: &[String], span: Span) {
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
            report_error(
                db,
                ValidationError::TooManyArguments {
                    tag: tag_name.to_string(),
                    max: 0,
                    span,
                },
            );
        }
        return;
    }

    let has_varargs = args.iter().any(|arg| matches!(arg, TagArg::VarArgs { .. }));
    let required_count = args.iter().filter(|arg| arg.is_required()).count();

    if bits.len() < required_count {
        report_error(
            db,
            ValidationError::MissingRequiredArguments {
                tag: tag_name.to_string(),
                min: required_count,
                span,
            },
        );
    }

    if !has_varargs && bits.len() > args.len() {
        report_error(
            db,
            ValidationError::TooManyArguments {
                tag: tag_name.to_string(),
                max: args.len(),
                span,
            },
        );
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
                report_error(
                    db,
                    ValidationError::InvalidLiteralArgument {
                        tag: tag_name.to_string(),
                        expected: lit.to_string(),
                        span,
                    },
                );
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
                        report_error(
                            db,
                            ValidationError::InvalidLiteralArgument {
                                tag: tag_name.to_string(),
                                expected: lit.to_string(),
                                span,
                            },
                        );
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
                    report_error(
                        db,
                        ValidationError::InvalidArgumentChoice {
                            tag: tag_name.to_string(),
                            argument: name.to_string(),
                            choices: choices.iter().map(|c| c.to_string()).collect(),
                            value: value.clone(),
                            span,
                        },
                    );
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
            if let Some(name) = argument_name(arg) {
                report_error(
                    db,
                    ValidationError::MissingArgument {
                        tag: tag_name.to_string(),
                        argument: name,
                        span,
                    },
                );
            }
        }
    }
}

fn argument_name(arg: &TagArg) -> Option<String> {
    match arg {
        TagArg::Literal { lit, .. } => Some(lit.to_string()),
        TagArg::Choice { name, .. }
        | TagArg::Var { name, .. }
        | TagArg::String { name, .. }
        | TagArg::Expr { name, .. }
        | TagArg::Assignment { name, .. }
        | TagArg::VarArgs { name, .. } => Some(name.to_string()),
    }
}

fn report_error(db: &dyn Db, error: ValidationError) {
    ValidationErrorAccumulator(error).accumulate(db);
}

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::OnceLock;

use djls_project::ArgumentCountConstraint;
use djls_project::ArgumentFormCoverage;
use djls_project::AssignmentMode;
use djls_project::AssignmentOperand;
use djls_project::ChoiceAt;
use djls_project::ExtractedDiagnosticConstraint;
use djls_project::ExtractedDiagnosticMessage;
use djls_project::ExtractedMessageArg;
use djls_project::ExtractedMessageTemplate;
use djls_project::FormAtomExpectation;
use djls_project::FormAtomMismatch;
use djls_project::KnownOptions;
use djls_project::OptionRejection;
use djls_project::RemainderPolicy;
use djls_project::RequiredKeyword;
use djls_project::SplitPosition;
use djls_project::TagArgumentForm;
use djls_project::TagArgumentFormMismatch;
use djls_project::TagArgumentKind;
use djls_project::TagArgumentSyntax;
use djls_project::TagRule;
use djls_project::UniqueKeyCardinality;
use djls_source::Span;
use regex::Regex;

use crate::errors::ValidationError;

trait Constraint {
    fn validate(
        &self,
        tag_name: &str,
        bits: &[String],
        span: Span,
        message: Option<String>,
    ) -> Option<ValidationError>;
}

/// Constraints express the conditions from Django source that raise exceptions
/// in guard patterns. The extraction captures what makes the tag **valid**:
/// - `Exact(N)`: valid when `split_len == N`
/// - `Min(N)`: valid when `split_len >= N`
/// - `Max(N)`: valid when `split_len <= N`
/// - `OneOf(set)`: valid when `split_len in set`
impl Constraint for ArgumentCountConstraint {
    fn validate(
        &self,
        tag_name: &str,
        bits: &[String],
        span: Span,
        message: Option<String>,
    ) -> Option<ValidationError> {
        let split_len = bits.len() + 1;

        let violated = match self {
            ArgumentCountConstraint::Exact(n) => split_len != *n,
            ArgumentCountConstraint::Min(n) => split_len < *n,
            ArgumentCountConstraint::Max(n) => split_len > *n,
            ArgumentCountConstraint::OneOf(values) => !values.contains(&split_len),
        };

        if violated {
            let message = message.unwrap_or_else(|| match self {
                ArgumentCountConstraint::Exact(n) => {
                    let expected_args = n.saturating_sub(1);
                    let actual_args = split_len.saturating_sub(1);
                    format!(
                        "Tag '{tag_name}' takes exactly {expected_args} argument{}, but {actual_args} {} given",
                        if expected_args == 1 { "" } else { "s" },
                        if actual_args == 1 { "was" } else { "were" }
                    )
                }
                ArgumentCountConstraint::Min(n) => {
                    let min_args = n.saturating_sub(1);
                    format!(
                        "Tag '{tag_name}' requires at least {min_args} argument{}",
                        if min_args == 1 { "" } else { "s" }
                    )
                }
                ArgumentCountConstraint::Max(n) => {
                    let max_args = n.saturating_sub(1);
                    format!(
                        "Tag '{tag_name}' accepts at most {max_args} argument{}",
                        if max_args == 1 { "" } else { "s" }
                    )
                }
                ArgumentCountConstraint::OneOf(values) => {
                    let arg_counts: Vec<String> = values
                        .iter()
                        .map(|v| v.saturating_sub(1).to_string())
                        .collect();
                    format!(
                        "Tag '{tag_name}' takes {} arguments",
                        arg_counts.join(" or ")
                    )
                }
            });

            Some(ValidationError::ExtractedRuleViolation {
                tag: tag_name.to_string(),
                message,
                span,
            })
        } else {
            None
        }
    }
}

impl Constraint for RequiredKeyword {
    fn validate(
        &self,
        tag_name: &str,
        bits: &[String],
        span: Span,
        message: Option<String>,
    ) -> Option<ValidationError> {
        let bits_index = self.position.to_bits_index(bits.len())?;
        let bit = bits.get(bits_index)?;

        if bit == &self.value {
            None
        } else {
            Some(ValidationError::ExtractedRuleViolation {
                tag: tag_name.to_string(),
                message: match message {
                    Some(message) => message,
                    None => format!(
                        "Tag '{tag_name}' expects '{}' at position {}",
                        self.value, self.position
                    ),
                },
                span,
            })
        }
    }
}

impl Constraint for ChoiceAt {
    fn validate(
        &self,
        tag_name: &str,
        bits: &[String],
        span: Span,
        message: Option<String>,
    ) -> Option<ValidationError> {
        let bits_index = self.position.to_bits_index(bits.len())?;
        let bit = bits.get(bits_index)?;

        if self.values.iter().any(|value| value == bit) {
            None
        } else {
            let choices = self.values.join("', '");
            Some(ValidationError::ExtractedRuleViolation {
                tag: tag_name.to_string(),
                message: match message {
                    Some(message) => message,
                    None => format!("Tag '{tag_name}' argument must be one of: '{choices}'"),
                },
                span,
            })
        }
    }
}

/// Evaluate extracted tag rules against template tag arguments.
///
/// `bits` is the tag's argument list as text, excluding the tag name. Extraction
/// rules use Django `split_contents()` indices where the tag name is at index 0,
/// so the evaluator adjusts by adding 1 to `bits.len()` when comparing against
/// `ArgumentCountConstraint` values, and subtracting 1 from
/// `RequiredKeyword.position` when indexing into `bits`.
#[must_use]
pub(crate) fn evaluate_tag_rules(
    tag_name: &str,
    bits: &[String],
    rules: &TagRule,
    span: Span,
) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    let effective_bits = effective_tag_bits(bits, rules.as_var.strips_suffix());

    let diagnostic_messages = rules.diagnostic_messages.as_deref().unwrap_or(&[]);

    if let Some(error) =
        validate_argument_syntax(tag_name, &rules.argument_syntax, effective_bits, span)
    {
        return vec![error];
    }

    for constraint in &rules.arg_constraints {
        let message = message_for_constraint(
            diagnostic_messages,
            &ExtractedDiagnosticConstraint::ArgumentCount(constraint.clone()),
            tag_name,
            effective_bits,
        );
        errors.extend(constraint.validate(tag_name, effective_bits, span, message));
    }

    // When multiple required_keywords target the same position with different
    // values (from different if/elif branches), treat them as alternatives:
    // at least one must match. Single keywords at a position remain strict.
    {
        let mut by_position: HashMap<&SplitPosition, Vec<&RequiredKeyword>> = HashMap::new();
        for keyword in &rules.required_keywords {
            by_position
                .entry(&keyword.position)
                .or_default()
                .push(keyword);
        }
        for keywords in by_position.values() {
            if keywords.len() == 1 {
                let keyword = keywords[0];
                let message = message_for_constraint(
                    diagnostic_messages,
                    &ExtractedDiagnosticConstraint::RequiredKeyword {
                        position: keyword.position,
                        value: keyword.value.clone(),
                    },
                    tag_name,
                    effective_bits,
                );
                errors.extend(keyword.validate(tag_name, effective_bits, span, message));
            } else {
                // Multiple keywords at the same position → OR semantics.
                // If any one matches, no error. If all fail, report the first.
                let all_fail = keywords
                    .iter()
                    .all(|kw| kw.validate(tag_name, effective_bits, span, None).is_some());
                if all_fail {
                    // Pick the first as representative error, but phrase it
                    // as a choice to be clearer.
                    let values: Vec<&str> = keywords.iter().map(|kw| kw.value.as_str()).collect();
                    let bits_index = keywords[0].position.to_bits_index(effective_bits.len());
                    if bits_index.is_some() {
                        let choices = values.join("' or '");
                        errors.push(ValidationError::ExtractedRuleViolation {
                            tag: tag_name.to_string(),
                            message: format!(
                                "Tag '{tag_name}' expects '{}' at position {}",
                                choices, keywords[0].position
                            ),
                            span,
                        });
                    }
                }
            }
        }
    }

    for choice in &rules.choice_at_constraints {
        let message = message_for_constraint(
            diagnostic_messages,
            &ExtractedDiagnosticConstraint::ChoiceAt {
                position: choice.position,
                values: choice.values.clone(),
            },
            tag_name,
            effective_bits,
        );
        errors.extend(choice.validate(tag_name, effective_bits, span, message));
    }

    if let Some(options) = &rules.known_options {
        errors.extend(evaluate_known_options(
            tag_name,
            effective_bits,
            options,
            span,
        ));
    }

    errors
}

struct AssignmentMatch {
    consumed: usize,
    unique_keys: usize,
}

fn validate_assignments(
    tag_name: &str,
    bits: &[String],
    operand: &AssignmentOperand,
    span: Span,
) -> Option<ValidationError> {
    let matched = match_assignments(bits, operand.mode);
    let cardinality_failed = match operand.cardinality {
        UniqueKeyCardinality::Any => false,
        UniqueKeyCardinality::AtLeastOne => matched.unique_keys < 1,
        UniqueKeyCardinality::ExactlyOne => matched.unique_keys != 1,
    };
    let message = if cardinality_failed {
        let source_message = if matched.unique_keys == 0 {
            &operand.empty_message
        } else {
            &operand.multiple_message
        };
        source_message
            .as_ref()
            .and_then(|message| render_message_template(message, tag_name, bits))
            .unwrap_or_else(|| {
                if operand.cardinality == UniqueKeyCardinality::AtLeastOne {
                    format!("'{tag_name}' expected at least one variable assignment")
                } else {
                    format!("'{tag_name}' expected exactly one variable assignment")
                }
            })
    } else if operand.remainder == RemainderPolicy::Reject && matched.consumed != bits.len() {
        operand
            .remainder_message
            .as_ref()
            .and_then(|message| render_message_template(message, tag_name, bits))
            .unwrap_or_else(|| format!("'{tag_name}' received an invalid assignment"))
    } else {
        return None;
    };
    Some(ValidationError::ExtractedRuleViolation {
        tag: tag_name.to_string(),
        message,
        span,
    })
}

fn match_assignments(bits: &[String], mode: AssignmentMode) -> AssignmentMatch {
    let mut keys = HashSet::new();
    let mut consumed = 0;
    if bits
        .first()
        .and_then(|bit| modern_assignment(bit))
        .is_some()
    {
        for bit in bits {
            let Some((key, _)) = modern_assignment(bit) else {
                break;
            };
            keys.insert(key);
            consumed += 1;
        }
    } else if mode == AssignmentMode::ModernOrLegacy {
        while bits.get(consumed + 1).is_some_and(|bit| bit == "as")
            && bits.get(consumed + 2).is_some()
        {
            if let Some(key) = bits.get(consumed + 2) {
                keys.insert(key.as_str());
            }
            consumed += 3;
            if bits.get(consumed).is_some_and(|bit| bit == "and") {
                consumed += 1;
            } else {
                break;
            }
        }
    }
    AssignmentMatch {
        consumed,
        unique_keys: keys.len(),
    }
}

fn modern_assignment(bit: &str) -> Option<(&str, &str)> {
    let (key, value) = bit.split_once('=')?;
    (!value.is_empty() && is_python_word_key(key)).then_some((key, value))
}

fn is_python_word_key(value: &str) -> bool {
    static PYTHON_WORD_KEY: OnceLock<Result<Regex, regex::Error>> = OnceLock::new();
    PYTHON_WORD_KEY
        .get_or_init(|| Regex::new(r"\A[\p{Letter}\p{Number}_]+\z"))
        .as_ref()
        .is_ok_and(|pattern| pattern.is_match(value))
}

fn effective_tag_bits(bits: &[String], strips_as_var: bool) -> &[String] {
    if strips_as_var && bits.len() >= 2 && bits[bits.len() - 2] == "as" {
        &bits[..bits.len() - 2]
    } else {
        bits
    }
}

fn validate_argument_syntax(
    tag_name: &str,
    syntax: &TagArgumentSyntax,
    bits: &[String],
    span: Span,
) -> Option<ValidationError> {
    match syntax {
        TagArgumentSyntax::Signature {
            parameters,
            variadic_keyword,
            ..
        } => validate_django_signature(
            tag_name,
            parameters,
            variadic_keyword.as_deref(),
            bits,
            span,
        ),
        TagArgumentSyntax::Assignments { operand } => {
            validate_assignments(tag_name, bits, operand, span)
        }
        TagArgumentSyntax::Forms {
            forms,
            coverage: ArgumentFormCoverage::Complete,
            length_mismatch_message,
        } => match_complete_forms(forms, bits).err().map(|mismatch| {
            form_mismatch_error(
                tag_name,
                bits,
                forms,
                length_mismatch_message.as_ref(),
                mismatch,
                span,
            )
        }),
        TagArgumentSyntax::Forms {
            coverage: ArgumentFormCoverage::Partial,
            ..
        }
        | TagArgumentSyntax::Parameters(_)
        | TagArgumentSyntax::Unknown => None,
    }
}

fn validate_django_signature(
    tag_name: &str,
    parameters: &[djls_project::TagArgument],
    variadic_keyword: Option<&str>,
    bits: &[String],
    span: Span,
) -> Option<ValidationError> {
    bind_django_signature(parameters, variadic_keyword, bits)
        .err()
        .map(|message| ValidationError::ExtractedRuleViolation {
            tag: tag_name.to_string(),
            message: format!("'{tag_name}' {message}"),
            span,
        })
}

fn bind_django_signature(
    parameters: &[djls_project::TagArgument],
    variadic_keyword: Option<&str>,
    bits: &[String],
) -> Result<(), String> {
    let mut unhandled_positional = parameters
        .iter()
        .take_while(|parameter| matches!(parameter.kind, TagArgumentKind::Variable))
        .map(|parameter| parameter.name.as_str())
        .collect::<Vec<_>>();
    let positional_names = unhandled_positional.clone();
    let has_varargs = parameters
        .iter()
        .any(|parameter| matches!(parameter.kind, TagArgumentKind::VarArgs));
    let keyword_only = parameters
        .iter()
        .filter(|parameter| matches!(parameter.kind, TagArgumentKind::Keyword))
        .collect::<Vec<_>>();
    let mut unhandled_keyword_only = keyword_only
        .iter()
        .filter(|parameter| parameter.requirement.is_required())
        .map(|parameter| parameter.name.as_str())
        .collect::<Vec<_>>();
    let default_count = parameters
        .iter()
        .take_while(|parameter| matches!(parameter.kind, TagArgumentKind::Variable))
        .filter(|parameter| !parameter.requirement.is_required())
        .count();
    let mut seen_keywords = Vec::new();

    for bit in bits {
        if let Some(name) = django_keyword_name(bit) {
            let known = positional_names.contains(&name)
                || keyword_only.iter().any(|parameter| parameter.name == name);
            if !known && variadic_keyword.is_none() {
                return Err(format!("received unexpected keyword argument '{name}'"));
            }
            if seen_keywords.contains(&name) {
                return Err(format!(
                    "received multiple values for keyword argument '{name}'"
                ));
            }
            seen_keywords.push(name);
            if let Some(index) = unhandled_positional
                .iter()
                .position(|parameter| *parameter == name)
            {
                unhandled_positional.remove(index);
            } else if let Some(index) = unhandled_keyword_only
                .iter()
                .position(|parameter| *parameter == name)
            {
                unhandled_keyword_only.remove(index);
            }
        } else if !seen_keywords.is_empty() {
            return Err(
                "received some positional argument(s) after some keyword argument(s)".to_string(),
            );
        } else if unhandled_positional.is_empty() {
            if !has_varargs {
                return Err("received too many positional arguments".to_string());
            }
        } else {
            unhandled_positional.remove(0);
        }
    }

    if default_count > 0 {
        unhandled_positional.truncate(unhandled_positional.len().saturating_sub(default_count));
    }
    if unhandled_positional.is_empty() && unhandled_keyword_only.is_empty() {
        return Ok(());
    }

    let missing = unhandled_positional
        .into_iter()
        .chain(unhandled_keyword_only)
        .map(|name| format!("'{name}'"))
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "did not receive value(s) for the argument(s): {missing}"
    ))
}

fn django_keyword_name(bit: &str) -> Option<&str> {
    let (name, value) = bit.split_once('=')?;
    (!value.is_empty() && is_python_word_key(name)).then_some(name)
}

fn match_complete_forms<'a>(
    forms: &'a [TagArgumentForm],
    bits: &[String],
) -> Result<(), TagArgumentFormMismatch<'a>> {
    let mut best_mismatch: Option<FormAtomMismatch<'a>> = None;
    for form in forms {
        match form.match_full(bits) {
            Ok(()) => return Ok(()),
            Err(TagArgumentFormMismatch::Length) => {}
            Err(TagArgumentFormMismatch::Atom(mismatch)) => {
                let candidate_is_exclusion =
                    matches!(mismatch.expected, FormAtomExpectation::Excluded(_));
                let replace = best_mismatch.is_none_or(|best| {
                    mismatch.argument_index > best.argument_index
                        || (mismatch.argument_index == best.argument_index
                            && !candidate_is_exclusion
                            && matches!(best.expected, FormAtomExpectation::Excluded(_)))
                });
                if replace {
                    best_mismatch = Some(mismatch);
                }
            }
        }
    }

    Err(best_mismatch.map_or(
        TagArgumentFormMismatch::Length,
        TagArgumentFormMismatch::Atom,
    ))
}

fn form_mismatch_error(
    tag_name: &str,
    bits: &[String],
    forms: &[TagArgumentForm],
    length_mismatch_message: Option<&ExtractedMessageTemplate>,
    mismatch: TagArgumentFormMismatch<'_>,
    span: Span,
) -> ValidationError {
    let message = match mismatch {
        TagArgumentFormMismatch::Atom(FormAtomMismatch {
            argument_index,
            expected,
            message,
        }) => {
            let forward = SplitPosition::Forward(argument_index + 1);
            message
                .and_then(|message| render_message_template(message, tag_name, bits))
                .unwrap_or_else(|| match expected {
                    FormAtomExpectation::Literal(value) => {
                        format!("Tag '{tag_name}' expects '{value}' at position {forward}")
                    }
                    FormAtomExpectation::Choice(values) => format!(
                        "Tag '{tag_name}' argument must be one of: '{}'",
                        values.join("', '")
                    ),
                    FormAtomExpectation::Excluded(value) => {
                        format!("Tag '{tag_name}' does not accept '{value}' at position {forward}")
                    }
                })
        }
        TagArgumentFormMismatch::Length => {
            if let Some(message) = length_mismatch_message
                .and_then(|message| render_message_template(message, tag_name, bits))
            {
                message
            } else {
                return form_length_mismatch_error(tag_name, bits.len(), forms, span);
            }
        }
    };
    ValidationError::ExtractedRuleViolation {
        tag: tag_name.to_string(),
        message,
        span,
    }
}

fn form_length_mismatch_error(
    tag_name: &str,
    argument_count: usize,
    forms: &[TagArgumentForm],
    span: Span,
) -> ValidationError {
    let constraint = if forms.iter().all(|form| form.exact_len().is_some()) {
        let mut split_lengths = forms
            .iter()
            .filter_map(TagArgumentForm::exact_len)
            .map(|length| length + 1)
            .collect::<Vec<_>>();
        split_lengths.sort_unstable();
        split_lengths.dedup();
        if let [length] = split_lengths.as_slice() {
            ArgumentCountConstraint::Exact(*length)
        } else {
            ArgumentCountConstraint::OneOf(split_lengths)
        }
    } else {
        ArgumentCountConstraint::Min(
            forms
                .iter()
                .map(TagArgumentForm::minimum_len)
                .min()
                .unwrap_or(0)
                + 1,
        )
    };
    let message = match &constraint {
        ArgumentCountConstraint::Exact(expected) => {
            let expected_args = expected.saturating_sub(1);
            format!(
                "Tag '{tag_name}' takes exactly {expected_args} argument{}, but {argument_count} {} given",
                if expected_args == 1 { "" } else { "s" },
                if argument_count == 1 { "was" } else { "were" }
            )
        }
        ArgumentCountConstraint::OneOf(values) => format!(
            "Tag '{tag_name}' takes {} arguments",
            values
                .iter()
                .map(|value| value.saturating_sub(1).to_string())
                .collect::<Vec<_>>()
                .join(" or ")
        ),
        ArgumentCountConstraint::Min(minimum) => format!(
            "Tag '{tag_name}' requires at least {} arguments",
            minimum.saturating_sub(1)
        ),
        ArgumentCountConstraint::Max(maximum) => format!(
            "Tag '{tag_name}' accepts at most {} arguments",
            maximum.saturating_sub(1)
        ),
    };
    ValidationError::ExtractedRuleViolation {
        tag: tag_name.to_string(),
        message,
        span,
    }
}

fn message_for_constraint(
    messages: &[ExtractedDiagnosticMessage],
    constraint: &ExtractedDiagnosticConstraint,
    tag_name: &str,
    bits: &[String],
) -> Option<String> {
    messages.iter().find_map(|message| {
        if &message.constraint == constraint {
            render_message_template(&message.message, tag_name, bits)
        } else {
            None
        }
    })
}

fn render_message_template(
    message: &ExtractedMessageTemplate,
    tag_name: &str,
    bits: &[String],
) -> Option<String> {
    match message {
        ExtractedMessageTemplate::Static(message) => Some(message.clone()),
        ExtractedMessageTemplate::PercentFormat { template, args } => {
            render_percent_format(template, args, tag_name, bits)
        }
    }
}

fn render_percent_format(
    template: &str,
    args: &[ExtractedMessageArg],
    tag_name: &str,
    bits: &[String],
) -> Option<String> {
    let mut rendered = String::new();
    let mut chars = template.chars().peekable();
    let mut args = args.iter();

    while let Some(ch) = chars.next() {
        if ch != '%' {
            rendered.push(ch);
            continue;
        }

        let spec = chars.next()?;
        match spec {
            '%' => rendered.push('%'),
            's' => rendered.push_str(&format_arg(
                args.next()?,
                tag_name,
                bits,
                FormatKind::String,
            )?),
            'r' => rendered.push_str(&format_arg(args.next()?, tag_name, bits, FormatKind::Repr)?),
            'd' | 'i' => {
                rendered.push_str(&format_arg(
                    args.next()?,
                    tag_name,
                    bits,
                    FormatKind::Integer,
                )?);
            }
            _ => return None,
        }
    }

    if args.next().is_some() {
        return None;
    }

    Some(rendered)
}

#[derive(Clone, Copy)]
enum FormatKind {
    String,
    Repr,
    Integer,
}

fn format_arg(
    arg: &ExtractedMessageArg,
    tag_name: &str,
    bits: &[String],
    kind: FormatKind,
) -> Option<String> {
    match (arg, kind) {
        (ExtractedMessageArg::Int(value), FormatKind::Integer) => Some(value.to_string()),
        (ExtractedMessageArg::Int(value), FormatKind::String | FormatKind::Repr) => {
            Some(value.to_string())
        }
        (ExtractedMessageArg::String(value), FormatKind::String) => Some(value.clone()),
        (ExtractedMessageArg::String(value), FormatKind::Repr) => Some(python_repr(value)),
        (
            ExtractedMessageArg::String(_)
            | ExtractedMessageArg::SplitElement(_)
            | ExtractedMessageArg::TokenContents,
            FormatKind::Integer,
        ) => None,
        (ExtractedMessageArg::TokenContents, FormatKind::String) => {
            Some(normalized_token_contents(tag_name, bits))
        }
        (ExtractedMessageArg::TokenContents, FormatKind::Repr) => {
            Some(python_repr(&normalized_token_contents(tag_name, bits)))
        }
        (ExtractedMessageArg::SplitElement(position), FormatKind::String) => {
            split_position_value(*position, tag_name, bits)
        }
        (ExtractedMessageArg::SplitElement(position), FormatKind::Repr) => {
            split_position_value(*position, tag_name, bits).map(|value| python_repr(&value))
        }
    }
}

fn normalized_token_contents(tag_name: &str, bits: &[String]) -> String {
    std::iter::once(tag_name)
        .chain(bits.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

fn split_position_value(
    position: SplitPosition,
    tag_name: &str,
    bits: &[String],
) -> Option<String> {
    match position {
        SplitPosition::Forward(0) => Some(tag_name.to_string()),
        SplitPosition::Forward(index) => bits.get(index - 1).cloned(),
        SplitPosition::Backward(index) => {
            if index == 0 || index > bits.len() {
                None
            } else {
                bits.get(bits.len() - index).cloned()
            }
        }
    }
}

fn python_repr(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// Evaluate known options constraints.
///
/// Scans `bits` for duplicates when extraction detected a rejection guard.
/// Unknown-option rejection remains a source fact: unknown bits may be
/// positional values, so validation needs tag-specific parsing context to use it.
fn evaluate_known_options(
    tag_name: &str,
    bits: &[String],
    options: &KnownOptions,
    span: Span,
) -> Vec<ValidationError> {
    match options.duplicate_rejection {
        OptionRejection::NotDetected => return Vec::new(),
        OptionRejection::Detected => {}
    }

    let mut errors = Vec::new();
    let mut seen = Vec::new();

    for bit in bits {
        let is_known = options.values.iter().any(|v| v == bit);

        if is_known {
            if seen.contains(bit) {
                errors.push(ValidationError::ExtractedRuleViolation {
                    tag: tag_name.to_string(),
                    message: format!("Tag '{tag_name}' received duplicate option '{bit}'"),
                    span,
                });
            }
            seen.push(bit.clone());
        }
    }

    errors
}

#[cfg(test)]
mod tests {
    use djls_project::AsVar;
    use djls_project::ParameterRequirement;
    use djls_project::SplitPosition;
    use djls_project::TagArgument;
    use djls_project::TagArgumentPattern;
    use djls_project::TagArgumentPatternKind;

    use super::*;

    fn make_bits(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| (*s).to_string()).collect()
    }

    fn form(kinds: Vec<TagArgumentKind>) -> TagArgumentForm {
        TagArgumentForm::new(
            kinds
                .into_iter()
                .enumerate()
                .map(|(index, kind)| TagArgumentPattern {
                    name: format!("arg{}", index + 1),
                    kind: match kind {
                        TagArgumentKind::Variable
                        | TagArgumentKind::VarArgs
                        | TagArgumentKind::Keyword => TagArgumentPatternKind::Variable,
                        TagArgumentKind::Literal(value) => TagArgumentPatternKind::Literal(value),
                        TagArgumentKind::Choice(values) => TagArgumentPatternKind::Choice(values),
                    },
                    mismatch_message: None,
                })
                .collect(),
        )
        .expect("fixed-width test forms are valid")
    }

    #[test]
    fn django_signature_binder_matches_parse_bits_order_and_default_quirks() {
        let parameters = vec![
            TagArgument {
                name: "one".to_string(),
                requirement: ParameterRequirement::Required,
                kind: TagArgumentKind::Variable,
            },
            TagArgument {
                name: "two".to_string(),
                requirement: ParameterRequirement::Optional,
                kind: TagArgumentKind::Variable,
            },
        ];

        assert_eq!(
            bind_django_signature(&parameters, None, &make_bits(&[])),
            Err("did not receive value(s) for the argument(s): 'one'".to_string())
        );
        assert_eq!(
            bind_django_signature(&parameters, None, &make_bits(&["two=value"])),
            Ok(())
        );
        assert_eq!(
            bind_django_signature(&parameters, None, &make_bits(&["first", "one=second"]),),
            Ok(())
        );
        assert_eq!(
            bind_django_signature(&parameters, None, &make_bits(&["one=first", "one=second"]),),
            Err("received multiple values for keyword argument 'one'".to_string())
        );
        assert_eq!(
            bind_django_signature(&parameters, None, &make_bits(&["one=first", "second"]),),
            Err("received some positional argument(s) after some keyword argument(s)".to_string())
        );
        assert_eq!(
            bind_django_signature(&parameters, None, &make_bits(&["unknown=value"])),
            Err("received unexpected keyword argument 'unknown'".to_string())
        );
        assert_eq!(
            bind_django_signature(&parameters, None, &make_bits(&["first", "second", "third"]),),
            Err("received too many positional arguments".to_string())
        );
    }

    #[test]
    fn django_signature_binder_handles_keyword_only_varargs_and_kwargs_independently() {
        let keyword_parameters = vec![
            TagArgument {
                name: "values".to_string(),
                requirement: ParameterRequirement::Optional,
                kind: TagArgumentKind::VarArgs,
            },
            TagArgument {
                name: "required".to_string(),
                requirement: ParameterRequirement::Required,
                kind: TagArgumentKind::Keyword,
            },
            TagArgument {
                name: "optional".to_string(),
                requirement: ParameterRequirement::Optional,
                kind: TagArgumentKind::Keyword,
            },
        ];
        assert_eq!(
            bind_django_signature(&keyword_parameters, None, &make_bits(&["first"])),
            Err("did not receive value(s) for the argument(s): 'required'".to_string())
        );
        assert_eq!(
            bind_django_signature(
                &keyword_parameters,
                None,
                &make_bits(&["first", "second", "required=value"]),
            ),
            Ok(())
        );

        let kwargs_parameters = vec![TagArgument {
            name: "one".to_string(),
            requirement: ParameterRequirement::Required,
            kind: TagArgumentKind::Variable,
        }];
        let variadic_keyword = Some("extra".to_string());
        assert_eq!(
            bind_django_signature(
                &kwargs_parameters,
                variadic_keyword.as_deref(),
                &make_bits(&["one=value", "unknown=value"]),
            ),
            Ok(())
        );
        assert_eq!(
            bind_django_signature(
                &kwargs_parameters,
                variadic_keyword.as_deref(),
                &make_bits(&["first", "second"]),
            ),
            Err("received too many positional arguments".to_string())
        );
    }

    #[test]
    fn manual_parameter_hints_are_not_bound() {
        let rule = TagRule {
            argument_syntax: TagArgumentSyntax::Parameters(vec![TagArgument {
                name: "required_hint".to_string(),
                requirement: ParameterRequirement::Required,
                kind: TagArgumentKind::Keyword,
            }]),
            ..TagRule::default()
        };

        assert!(evaluate_tag_rules("manual", &[], &rule, Span::new(0, 1)).is_empty());
    }

    // --- ArgumentCountConstraint tests ---

    #[test]
    fn exact_constraint_passes_when_matched() {
        let rule = TagRule {
            arg_constraints: vec![ArgumentCountConstraint::Exact(4)],
            ..TagRule::default()
        };
        // 3 bits + tag name = split_len 4
        let bits = make_bits(&["item", "in", "items"]);
        let errors = evaluate_tag_rules("for", &bits, &rule, Span::new(0, 10));
        assert!(errors.is_empty());
    }

    #[test]
    fn exact_constraint_fails_when_wrong_count() {
        let rule = TagRule {
            arg_constraints: vec![ArgumentCountConstraint::Exact(4)],
            ..TagRule::default()
        };
        // 2 bits + tag name = split_len 3, expected 4
        let bits = make_bits(&["item", "in"]);
        let errors = evaluate_tag_rules("for", &bits, &rule, Span::new(0, 10));
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            &errors[0],
            ValidationError::ExtractedRuleViolation { tag, message, .. }
            if tag == "for" && message.contains("exactly 3 argument")
        ));
    }

    #[test]
    fn complete_forms_validate_alternatives_without_cross_products() {
        let rule = TagRule {
            argument_syntax: TagArgumentSyntax::Forms {
                forms: vec![
                    form(vec![
                        TagArgumentKind::Literal("first".into()),
                        TagArgumentKind::Variable,
                        TagArgumentKind::Literal("left".into()),
                    ]),
                    form(vec![
                        TagArgumentKind::Literal("second".into()),
                        TagArgumentKind::Variable,
                        TagArgumentKind::Literal("right".into()),
                    ]),
                ],
                coverage: ArgumentFormCoverage::Complete,
                length_mismatch_message: None,
            },
            ..TagRule::default()
        };

        for bits in [
            make_bits(&["first", "value", "left"]),
            make_bits(&["second", "value", "right"]),
        ] {
            assert!(evaluate_tag_rules("custom", &bits, &rule, Span::new(0, 10)).is_empty());
        }
        let crossed = make_bits(&["first", "value", "right"]);
        assert_eq!(
            evaluate_tag_rules("custom", &crossed, &rule, Span::new(0, 10)).len(),
            1
        );
    }

    #[test]
    fn singleton_complete_form_rejects_excluded_value_at_correct_count() {
        let form = TagArgumentForm::new(vec![TagArgumentPattern {
            name: "value".to_string(),
            kind: TagArgumentPatternKind::VariableExcept("blocked".to_string()),
            mismatch_message: None,
        }])
        .expect("a fixed-width excluded-value form is valid");
        let rule = TagRule {
            argument_syntax: TagArgumentSyntax::Forms {
                forms: vec![form],
                coverage: ArgumentFormCoverage::Complete,
                length_mismatch_message: None,
            },
            ..TagRule::default()
        };

        let errors =
            evaluate_tag_rules("custom", &make_bits(&["blocked"]), &rule, Span::new(0, 10));

        assert!(matches!(
            errors.as_slice(),
            [ValidationError::ExtractedRuleViolation { tag, message, .. }]
                if tag == "custom"
                    && message == "Tag 'custom' does not accept 'blocked' at position 1"
        ));
    }

    #[test]
    fn partial_forms_do_not_reject_unknown_successful_syntax() {
        let rule = TagRule {
            argument_syntax: TagArgumentSyntax::Forms {
                forms: vec![form(vec![TagArgumentKind::Literal("known".into())])],
                coverage: ArgumentFormCoverage::Partial,
                length_mismatch_message: None,
            },
            ..TagRule::default()
        };
        let unknown = make_bits(&["other", "shape"]);
        assert!(evaluate_tag_rules("custom", &unknown, &rule, Span::new(0, 10)).is_empty());
    }

    #[test]
    fn extracted_static_message_overrides_generic_constraint_message() {
        let rule = TagRule {
            arg_constraints: vec![ArgumentCountConstraint::Exact(2)],
            diagnostic_messages: Some(vec![ExtractedDiagnosticMessage {
                constraint: ExtractedDiagnosticConstraint::ArgumentCount(
                    ArgumentCountConstraint::Exact(2),
                ),
                message: ExtractedMessageTemplate::Static(
                    "'custom' tag takes one argument".to_string(),
                ),
            }]),
            ..TagRule::default()
        };
        let errors = evaluate_tag_rules("custom", &[], &rule, Span::new(0, 10));
        assert!(matches!(
            &errors[0],
            ValidationError::ExtractedRuleViolation { message, .. }
            if message == "'custom' tag takes one argument"
        ));
    }

    #[test]
    fn extracted_percent_message_renders_runtime_tag_name() {
        let rule = TagRule {
            arg_constraints: vec![ArgumentCountConstraint::Min(2)],
            diagnostic_messages: Some(vec![ExtractedDiagnosticMessage {
                constraint: ExtractedDiagnosticConstraint::ArgumentCount(
                    ArgumentCountConstraint::Min(2),
                ),
                message: ExtractedMessageTemplate::PercentFormat {
                    template: "'%s' takes at least one argument".to_string(),
                    args: vec![ExtractedMessageArg::SplitElement(SplitPosition::Forward(0))],
                },
            }]),
            ..TagRule::default()
        };
        let errors = evaluate_tag_rules("custom", &[], &rule, Span::new(0, 10));
        assert!(matches!(
            &errors[0],
            ValidationError::ExtractedRuleViolation { message, .. }
            if message == "'custom' takes at least one argument"
        ));
    }

    #[test]
    fn token_contents_percent_format_uses_normalized_source_spelling() {
        let bits = make_bits(&["item", "from", "\"quoted value\""]);
        assert_eq!(
            render_percent_format(
                "bad %% tag: %s / %r",
                &[
                    ExtractedMessageArg::TokenContents,
                    ExtractedMessageArg::TokenContents,
                ],
                "targetTag",
                &bits,
            ),
            Some(
                "bad % tag: targetTag item from \"quoted value\" / 'targetTag item from \"quoted value\"'"
                    .to_string()
            )
        );
    }

    #[test]
    fn token_contents_integer_format_falls_back_to_generic_diagnostic() {
        let rule = TagRule {
            arg_constraints: vec![ArgumentCountConstraint::Exact(2)],
            diagnostic_messages: Some(vec![ExtractedDiagnosticMessage {
                constraint: ExtractedDiagnosticConstraint::ArgumentCount(
                    ArgumentCountConstraint::Exact(2),
                ),
                message: ExtractedMessageTemplate::PercentFormat {
                    template: "bad tag: %d".to_string(),
                    args: vec![ExtractedMessageArg::TokenContents],
                },
            }]),
            ..TagRule::default()
        };
        let errors = evaluate_tag_rules("targetTag", &[], &rule, Span::new(0, 10));

        assert!(matches!(
            &errors[0],
            ValidationError::ExtractedRuleViolation { message, .. }
                if message == "Tag 'targetTag' takes exactly 1 argument, but 0 were given"
        ));
    }

    #[test]
    fn min_constraint_passes_when_sufficient() {
        let rule = TagRule {
            arg_constraints: vec![ArgumentCountConstraint::Min(2)],
            ..TagRule::default()
        };
        let bits = make_bits(&["arg1", "arg2"]);
        let errors = evaluate_tag_rules("mytag", &bits, &rule, Span::new(0, 10));
        assert!(errors.is_empty());
    }

    #[test]
    fn min_constraint_fails_when_too_few() {
        let rule = TagRule {
            arg_constraints: vec![ArgumentCountConstraint::Min(4)],
            ..TagRule::default()
        };
        let bits = make_bits(&["arg1"]);
        let errors = evaluate_tag_rules("mytag", &bits, &rule, Span::new(0, 10));
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            &errors[0],
            ValidationError::ExtractedRuleViolation { message, .. }
            if message.contains("at least 3")
        ));
    }

    #[test]
    fn max_constraint_passes_when_under() {
        let rule = TagRule {
            arg_constraints: vec![ArgumentCountConstraint::Max(5)],
            ..TagRule::default()
        };
        let bits = make_bits(&["a", "b", "c"]);
        let errors = evaluate_tag_rules("mytag", &bits, &rule, Span::new(0, 10));
        assert!(errors.is_empty());
    }

    #[test]
    fn max_constraint_fails_when_over() {
        let rule = TagRule {
            arg_constraints: vec![ArgumentCountConstraint::Max(3)],
            ..TagRule::default()
        };
        let bits = make_bits(&["a", "b", "c"]);
        // split_len = 4, max = 3
        let errors = evaluate_tag_rules("mytag", &bits, &rule, Span::new(0, 10));
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            &errors[0],
            ValidationError::ExtractedRuleViolation { message, .. }
            if message.contains("at most 2")
        ));
    }

    #[test]
    fn one_of_constraint_passes_when_in_set() {
        let rule = TagRule {
            arg_constraints: vec![ArgumentCountConstraint::OneOf(vec![2, 4, 6])],
            ..TagRule::default()
        };
        // split_len = 4 (3 bits + tag name)
        let bits = make_bits(&["a", "b", "c"]);
        let errors = evaluate_tag_rules("mytag", &bits, &rule, Span::new(0, 10));
        assert!(errors.is_empty());
    }

    #[test]
    fn one_of_constraint_fails_when_not_in_set() {
        let rule = TagRule {
            arg_constraints: vec![ArgumentCountConstraint::OneOf(vec![2, 4])],
            ..TagRule::default()
        };
        // split_len = 3 (2 bits + tag name), not in {2, 4}
        let bits = make_bits(&["a", "b"]);
        let errors = evaluate_tag_rules("mytag", &bits, &rule, Span::new(0, 10));
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            &errors[0],
            ValidationError::ExtractedRuleViolation { message, .. }
            if message.contains("1 or 3 argument")
        ));
    }

    // --- RequiredKeyword tests ---

    #[test]
    fn required_keyword_passes_when_present() {
        let rule = TagRule {
            required_keywords: vec![RequiredKeyword {
                position: SplitPosition::Forward(2),
                value: "in".to_string(),
            }],
            ..TagRule::default()
        };
        // bits[1] (position 2 in split_contents - 1) = "in"
        let bits = make_bits(&["item", "in", "items"]);
        let errors = evaluate_tag_rules("for", &bits, &rule, Span::new(0, 10));
        assert!(errors.is_empty());
    }

    #[test]
    fn required_keyword_fails_when_wrong() {
        let rule = TagRule {
            required_keywords: vec![RequiredKeyword {
                position: SplitPosition::Forward(2),
                value: "in".to_string(),
            }],
            ..TagRule::default()
        };
        let bits = make_bits(&["item", "from", "items"]);
        let errors = evaluate_tag_rules("for", &bits, &rule, Span::new(0, 10));
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            &errors[0],
            ValidationError::ExtractedRuleViolation { message, .. }
            if message.contains("'in'") && message.contains("position 2")
        ));
    }

    #[test]
    fn required_keyword_negative_position() {
        let rule = TagRule {
            required_keywords: vec![RequiredKeyword {
                position: SplitPosition::Backward(2),
                value: "as".to_string(),
            }],
            ..TagRule::default()
        };
        // bits = ["'view_name'", "arg1", "as", "varname"]
        // bits[-2] = "as"
        let bits = make_bits(&["'view_name'", "arg1", "as", "varname"]);
        let errors = evaluate_tag_rules("url", &bits, &rule, Span::new(0, 10));
        assert!(errors.is_empty());
    }

    #[test]
    fn required_keyword_negative_position_fails() {
        let rule = TagRule {
            required_keywords: vec![RequiredKeyword {
                position: SplitPosition::Backward(2),
                value: "as".to_string(),
            }],
            ..TagRule::default()
        };
        let bits = make_bits(&["'view_name'", "arg1", "with", "varname"]);
        let errors = evaluate_tag_rules("url", &bits, &rule, Span::new(0, 10));
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn required_keyword_out_of_bounds_skipped() {
        let rule = TagRule {
            required_keywords: vec![RequiredKeyword {
                position: SplitPosition::Forward(5),
                value: "in".to_string(),
            }],
            ..TagRule::default()
        };
        let bits = make_bits(&["item"]);
        let errors = evaluate_tag_rules("for", &bits, &rule, Span::new(0, 10));
        assert!(errors.is_empty(), "Out-of-bounds keyword should be skipped");
    }

    #[test]
    fn required_keyword_position_zero_skipped() {
        let rule = TagRule {
            required_keywords: vec![RequiredKeyword {
                position: SplitPosition::Forward(0),
                value: "for".to_string(),
            }],
            ..TagRule::default()
        };
        let bits = make_bits(&["item", "in", "items"]);
        let errors = evaluate_tag_rules("for", &bits, &rule, Span::new(0, 10));
        assert!(errors.is_empty(), "Position 0 (tag name) should be skipped");
    }

    // --- KnownOptions tests ---

    #[test]
    fn known_options_no_duplicates_detected() {
        let rule = TagRule {
            known_options: Some(KnownOptions {
                values: vec!["only".to_string(), "with".to_string()],
                duplicate_rejection: OptionRejection::Detected,
                unknown_rejection: OptionRejection::Detected,
            }),
            ..TagRule::default()
        };
        let bits = make_bits(&["'template.html'", "with", "x=1", "with", "y=2"]);
        let errors = evaluate_tag_rules("include", &bits, &rule, Span::new(0, 10));
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            &errors[0],
            ValidationError::ExtractedRuleViolation { message, .. }
            if message.contains("duplicate") && message.contains("with")
        ));
    }

    #[test]
    fn known_options_without_duplicate_rejection_produce_no_error() {
        let rule = TagRule {
            known_options: Some(KnownOptions {
                values: vec!["only".to_string(), "with".to_string()],
                duplicate_rejection: OptionRejection::NotDetected,
                unknown_rejection: OptionRejection::Detected,
            }),
            ..TagRule::default()
        };
        let bits = make_bits(&["'template.html'", "with", "x=1", "with", "y=2"]);
        let errors = evaluate_tag_rules("include", &bits, &rule, Span::new(0, 10));
        assert!(errors.is_empty());
    }

    // --- Combined tests ---

    #[test]
    fn empty_rules_no_errors() {
        let rule = TagRule::default();
        let bits = make_bits(&["anything", "goes", "here"]);
        let errors = evaluate_tag_rules("mytag", &bits, &rule, Span::new(0, 10));
        assert!(errors.is_empty());
    }

    #[test]
    fn multiple_constraints_all_checked() {
        let rule = TagRule {
            arg_constraints: vec![
                ArgumentCountConstraint::Min(4),
                ArgumentCountConstraint::Max(6),
            ],
            required_keywords: vec![RequiredKeyword {
                position: SplitPosition::Forward(2),
                value: "in".to_string(),
            }],
            ..Default::default()
        };
        // split_len = 5 (4 bits + tag name), satisfies Min(4) and Max(6)
        // bits[1] = "in", satisfies keyword
        let bits = make_bits(&["item", "in", "items", "reversed"]);
        let errors = evaluate_tag_rules("for", &bits, &rule, Span::new(0, 10));
        assert!(errors.is_empty());
    }

    #[test]
    fn multiple_constraints_both_fail() {
        let rule = TagRule {
            arg_constraints: vec![ArgumentCountConstraint::Min(4)],
            required_keywords: vec![RequiredKeyword {
                position: SplitPosition::Forward(2),
                value: "in".to_string(),
            }],
            ..Default::default()
        };
        // split_len = 3, fails Min(4); bits[1] = "from", fails keyword
        let bits = make_bits(&["item", "from"]);
        let errors = evaluate_tag_rules("for", &bits, &rule, Span::new(0, 10));
        assert_eq!(errors.len(), 2);
    }

    #[test]
    fn index_offset_correctness() {
        // Extraction says position 2 in split_contents = bits[1]
        let rule = TagRule {
            required_keywords: vec![RequiredKeyword {
                position: SplitPosition::Forward(2),
                value: "in".to_string(),
            }],
            ..TagRule::default()
        };

        // {% for item in items %} → bits = ["item", "in", "items"]
        // split_contents = ["for", "item", "in", "items"]
        // position 2 in split_contents = "in" = bits[1] ✓
        let bits = make_bits(&["item", "in", "items"]);
        let errors = evaluate_tag_rules("for", &bits, &rule, Span::new(0, 10));
        assert!(
            errors.is_empty(),
            "Position 2 in split_contents should map to bits[1]"
        );
    }

    // --- as_var tests ---

    #[test]
    fn simple_tag_as_varname_passes_max_constraint() {
        // simple_tag with Max(2): accepts 1 arg, but `as varname` adds 2 more tokens
        let rule = TagRule {
            arg_constraints: vec![
                ArgumentCountConstraint::Min(2),
                ArgumentCountConstraint::Max(2),
            ],
            as_var: AsVar::Strip,
            ..Default::default()
        };
        // {% user_display user as foo %} → bits = ["user", "as", "foo"]
        let bits = make_bits(&["user", "as", "foo"]);
        let errors = evaluate_tag_rules("user_display", &bits, &rule, Span::new(0, 10));
        assert!(
            errors.is_empty(),
            "simple_tag with `as varname` should pass: {errors:?}"
        );
    }

    #[test]
    fn simple_tag_as_varname_zero_params() {
        // simple_tag with Max(1): accepts 0 args, `{% tag as foo %}`
        let rule = TagRule {
            arg_constraints: vec![ArgumentCountConstraint::Max(1)],
            as_var: AsVar::Strip,
            ..Default::default()
        };
        // {% get_providers as providers %} → bits = ["as", "providers"]
        let bits = make_bits(&["as", "providers"]);
        let errors = evaluate_tag_rules("get_providers", &bits, &rule, Span::new(0, 10));
        assert!(
            errors.is_empty(),
            "simple_tag with 0 params + `as varname` should pass: {errors:?}"
        );
    }

    #[test]
    fn simple_tag_without_as_still_validated() {
        // simple_tag with Max(2): accepts 1 arg, no `as` form
        let rule = TagRule {
            arg_constraints: vec![ArgumentCountConstraint::Max(2)],
            as_var: AsVar::Strip,
            ..Default::default()
        };
        // {% user_display user %} → bits = ["user"]
        let bits = make_bits(&["user"]);
        let errors = evaluate_tag_rules("user_display", &bits, &rule, Span::new(0, 10));
        assert!(errors.is_empty(), "Normal usage should pass: {errors:?}");
    }

    #[test]
    fn simple_tag_extra_args_still_rejected() {
        // simple_tag with Max(2): accepts 1 arg, extra args should fail
        let rule = TagRule {
            arg_constraints: vec![ArgumentCountConstraint::Max(2)],
            as_var: AsVar::Strip,
            ..Default::default()
        };
        // {% user_display user extra %} → bits = ["user", "extra"]
        let bits = make_bits(&["user", "extra"]);
        let errors = evaluate_tag_rules("user_display", &bits, &rule, Span::new(0, 10));
        assert_eq!(errors.len(), 1, "Extra args without `as` should still fail");
    }

    #[test]
    fn non_simple_tag_as_varname_not_stripped() {
        // Manual tag with AsVar::Keep: `as` is NOT stripped
        let rule = TagRule {
            arg_constraints: vec![ArgumentCountConstraint::Max(2)],
            as_var: AsVar::Keep,
            ..Default::default()
        };
        // bits = ["user", "as", "foo"] → split_len=4, Max(2) fails
        let bits = make_bits(&["user", "as", "foo"]);
        let errors = evaluate_tag_rules("mytag", &bits, &rule, Span::new(0, 10));
        assert_eq!(
            errors.len(),
            1,
            "Non-simple_tag should not strip `as varname`"
        );
    }

    // --- ChoiceAt tests ---

    #[test]
    fn choice_at_passes_when_valid() {
        let rule = TagRule {
            choice_at_constraints: vec![ChoiceAt {
                position: SplitPosition::Forward(1),
                values: vec!["on".to_string(), "off".to_string()],
            }],
            ..TagRule::default()
        };
        let bits = make_bits(&["on"]);
        let errors = evaluate_tag_rules("autoescape", &bits, &rule, Span::new(0, 10));
        assert!(errors.is_empty());
    }

    #[test]
    fn choice_at_fails_when_invalid() {
        let rule = TagRule {
            choice_at_constraints: vec![ChoiceAt {
                position: SplitPosition::Forward(1),
                values: vec!["on".to_string(), "off".to_string()],
            }],
            ..TagRule::default()
        };
        let bits = make_bits(&["unknown"]);
        let errors = evaluate_tag_rules("autoescape", &bits, &rule, Span::new(0, 10));
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            &errors[0],
            ValidationError::ExtractedRuleViolation { message, .. }
            if message.contains("'on', 'off'")
        ));
    }

    #[test]
    fn choice_at_negative_position() {
        let rule = TagRule {
            choice_at_constraints: vec![ChoiceAt {
                position: SplitPosition::Backward(1),
                values: vec!["yes".to_string(), "no".to_string()],
            }],
            ..TagRule::default()
        };
        // bits[-1] = "yes"
        let bits = make_bits(&["something", "yes"]);
        let errors = evaluate_tag_rules("mytag", &bits, &rule, Span::new(0, 10));
        assert!(errors.is_empty());
    }

    #[test]
    fn choice_at_out_of_bounds_skipped() {
        let rule = TagRule {
            choice_at_constraints: vec![ChoiceAt {
                position: SplitPosition::Forward(5),
                values: vec!["a".to_string()],
            }],
            ..TagRule::default()
        };
        let bits = make_bits(&["x"]);
        let errors = evaluate_tag_rules("mytag", &bits, &rule, Span::new(0, 10));
        assert!(errors.is_empty());
    }

    #[test]
    fn assignment_matching_uses_python_word_categories() {
        let modern = |bits: &[&str]| {
            let owned = bits.iter().map(ToString::to_string).collect::<Vec<_>>();
            match_assignments(&owned, AssignmentMode::Modern)
        };
        assert_eq!(modern(&["\u{30000}=1"]).consumed, 1);
        assert_eq!(modern(&["\u{0345}=1"]).consumed, 0);
        assert_eq!(modern(&["\u{1885}=1"]).consumed, 0);
    }

    #[test]
    fn assignment_cardinality_diagnostics_distinguish_empty_and_multiple_keys() {
        let operand = AssignmentOperand {
            mode: AssignmentMode::Modern,
            cardinality: UniqueKeyCardinality::ExactlyOne,
            remainder: RemainderPolicy::Reject,
            empty_message: Some(ExtractedMessageTemplate::Static(
                "needs an assignment".into(),
            )),
            multiple_message: Some(ExtractedMessageTemplate::Static(
                "too many assignments".into(),
            )),
            remainder_message: None,
        };
        for (bits, message) in [
            (make_bits(&[]), "needs an assignment"),
            (make_bits(&["x=1", "y=2"]), "too many assignments"),
        ] {
            let error = validate_assignments("assign", &bits, &operand, Span::new(0, 10));
            assert!(
                matches!(error, Some(ValidationError::ExtractedRuleViolation { message: actual, .. })
                if actual == message)
            );
        }
        assert!(
            validate_assignments(
                "assign",
                &make_bits(&["x=1", "x=2"]),
                &operand,
                Span::new(0, 10)
            )
            .is_none()
        );
    }

    #[test]
    fn assignment_matching_models_modern_legacy_and_remainder() {
        let owned =
            ["first", "as", "key", "and", "second", "as", "other", "and"].map(ToString::to_string);
        let legacy = match_assignments(&owned, AssignmentMode::ModernOrLegacy);
        assert_eq!(legacy.consumed, owned.len());
        assert_eq!(legacy.unique_keys, 2);

        let duplicate =
            ["first", "as", "key", "and", "second", "as", "key"].map(ToString::to_string);
        assert_eq!(
            match_assignments(&duplicate, AssignmentMode::ModernOrLegacy).unique_keys,
            1
        );

        let mixed = ["modern=first", "second", "as", "legacy"].map(ToString::to_string);
        assert_eq!(
            match_assignments(&mixed, AssignmentMode::ModernOrLegacy).consumed,
            1
        );
    }

    #[test]
    fn choice_at_combined_with_arg_count() {
        let rule = TagRule {
            arg_constraints: vec![ArgumentCountConstraint::Exact(2)],
            choice_at_constraints: vec![ChoiceAt {
                position: SplitPosition::Forward(1),
                values: vec!["on".to_string(), "off".to_string()],
            }],
            ..TagRule::default()
        };
        // Correct count, wrong value
        let bits = make_bits(&["bad"]);
        let errors = evaluate_tag_rules("autoescape", &bits, &rule, Span::new(0, 10));
        assert_eq!(errors.len(), 1); // Only choice violation, count is correct
        assert!(matches!(
            &errors[0],
            ValidationError::ExtractedRuleViolation { message, .. }
            if message.contains("must be one of")
        ));
    }
}

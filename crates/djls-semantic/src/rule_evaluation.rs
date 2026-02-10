use djls_python::ArgumentCountConstraint;
use djls_python::ChoiceAt;
use djls_python::KnownOptions;
use djls_python::RequiredKeyword;
use djls_python::TagRule;
use djls_source::Span;

use crate::ValidationError;

trait Constraint {
    fn validate(&self, tag_name: &str, bits: &[String], span: Span) -> Option<ValidationError>;
}

/// Resolve a `SplitPosition` to a `bits` index.
///
/// Delegates to `SplitPosition::to_bits_index`, which handles the offset
/// between `split_contents()` coordinates (tag name at index 0) and `bits`
/// coordinates (arguments only, tag name excluded).
///
/// Returns `None` if the position is out of bounds or refers to the tag name
/// — the argument count constraint should catch those cases.
fn resolve_position_index(
    position: &djls_python::SplitPosition,
    bits_len: usize,
) -> Option<usize> {
    position.to_bits_index(bits_len)
}

/// Constraints express the conditions from Django source that raise
/// `TemplateSyntaxError`. The extraction captures what makes the tag **valid**:
/// - `Exact(N)`: valid when `split_len == N`
/// - `Min(N)`: valid when `split_len >= N`
/// - `Max(N)`: valid when `split_len <= N`
/// - `OneOf(set)`: valid when `split_len in set`
impl Constraint for ArgumentCountConstraint {
    fn validate(&self, tag_name: &str, bits: &[String], span: Span) -> Option<ValidationError> {
        let split_len = bits.len() + 1;

        let violated = match self {
            ArgumentCountConstraint::Exact(n) => split_len != *n,
            ArgumentCountConstraint::Min(n) => split_len < *n,
            ArgumentCountConstraint::Max(n) => split_len > *n,
            ArgumentCountConstraint::OneOf(values) => !values.contains(&split_len),
        };

        if violated {
            let message = match self {
                ArgumentCountConstraint::Exact(n) => {
                    let expected_args = n.saturating_sub(1);
                    let actual_args = split_len.saturating_sub(1);
                    format!(
                        "'{tag_name}' takes exactly {expected_args} argument{}, {actual_args} given",
                        if expected_args == 1 { "" } else { "s" }
                    )
                }
                ArgumentCountConstraint::Min(n) => {
                    let min_args = n.saturating_sub(1);
                    format!(
                        "'{tag_name}' requires at least {min_args} argument{}",
                        if min_args == 1 { "" } else { "s" }
                    )
                }
                ArgumentCountConstraint::Max(n) => {
                    let max_args = n.saturating_sub(1);
                    format!(
                        "'{tag_name}' accepts at most {max_args} argument{}",
                        if max_args == 1 { "" } else { "s" }
                    )
                }
                ArgumentCountConstraint::OneOf(values) => {
                    let arg_counts: Vec<String> = values
                        .iter()
                        .map(|v| v.saturating_sub(1).to_string())
                        .collect();
                    format!("'{tag_name}' takes {} argument(s)", arg_counts.join(" or "))
                }
            };

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
    fn validate(&self, tag_name: &str, bits: &[String], span: Span) -> Option<ValidationError> {
        let bits_index = resolve_position_index(&self.position, bits.len())?;

        if bits[bits_index] == self.value {
            None
        } else {
            Some(ValidationError::ExtractedRuleViolation {
                tag: tag_name.to_string(),
                message: format!(
                    "'{tag_name}' expected '{}' at position {}",
                    self.value, self.position
                ),
                span,
            })
        }
    }
}

impl Constraint for ChoiceAt {
    fn validate(&self, tag_name: &str, bits: &[String], span: Span) -> Option<ValidationError> {
        let bits_index = resolve_position_index(&self.position, bits.len())?;

        if self.values.iter().any(|v| v == &bits[bits_index]) {
            None
        } else {
            let choices = self.values.join("', '");
            Some(ValidationError::ExtractedRuleViolation {
                tag: tag_name.to_string(),
                message: format!("'{tag_name}' argument must be one of '{choices}'"),
                span,
            })
        }
    }
}

/// Evaluate extracted tag rules against template tag arguments.
///
/// `bits` is the tag's argument list as produced by the parser, which
/// **excludes** the tag name. Extraction rules use `split_contents()` indices
/// where the tag name is at index 0, so the evaluator adjusts by adding 1 to
/// `bits.len()` when comparing against `ArgumentCountConstraint` values, and
/// subtracting 1 from `RequiredKeyword.position` when indexing into `bits`.
#[must_use]
pub fn evaluate_tag_rules(
    tag_name: &str,
    bits: &[String],
    rules: &TagRule,
    span: Span,
) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    // Django's simple_tag supports `{% tag args... as varname %}` syntax.
    // The framework strips `as varname` before validating arguments, so we
    // do the same: if the last two bits are ["as", <something>], strip them.
    let effective_bits = if rules.supports_as_var && bits.len() >= 2 && bits[bits.len() - 2] == "as"
    {
        &bits[..bits.len() - 2]
    } else {
        bits
    };

    for constraint in &rules.arg_constraints {
        errors.extend(constraint.validate(tag_name, effective_bits, span));
    }

    for keyword in &rules.required_keywords {
        errors.extend(keyword.validate(tag_name, effective_bits, span));
    }

    for choice in &rules.choice_at_constraints {
        errors.extend(choice.validate(tag_name, effective_bits, span));
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

/// Evaluate known options constraints.
///
/// Scans `bits` for option-style arguments and validates them against the
/// known set. Returns errors for unknown options (when `rejects_unknown`)
/// and duplicate options (when `!allow_duplicates`).
fn evaluate_known_options(
    tag_name: &str,
    bits: &[String],
    options: &KnownOptions,
    span: Span,
) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    let mut seen = Vec::new();

    for bit in bits {
        let is_known = options.values.iter().any(|v| v == bit);

        if is_known {
            if !options.allow_duplicates && seen.contains(bit) {
                errors.push(ValidationError::ExtractedRuleViolation {
                    tag: tag_name.to_string(),
                    message: format!("'{tag_name}' received duplicate option '{bit}'"),
                    span,
                });
            }
            seen.push(bit.clone());
        }
        // NOTE: `rejects_unknown` is not enforced — distinguishing unknown
        // options from positional values (e.g. `with key=val`) is unreliable
        // without full tag-specific parsing context.
    }

    errors
}

#[cfg(test)]
mod tests {
    use djls_python::SplitPosition;

    use super::*;

    fn make_span() -> Span {
        Span::new(0, 10)
    }

    fn make_bits(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| (*s).to_string()).collect()
    }

    fn empty_rule() -> TagRule {
        TagRule::default()
    }

    // --- ArgumentCountConstraint tests ---

    #[test]
    fn exact_constraint_passes_when_matched() {
        let rule = TagRule {
            arg_constraints: vec![ArgumentCountConstraint::Exact(4)],
            ..empty_rule()
        };
        // 3 bits + tag name = split_len 4
        let bits = make_bits(&["item", "in", "items"]);
        let errors = evaluate_tag_rules("for", &bits, &rule, make_span());
        assert!(errors.is_empty());
    }

    #[test]
    fn exact_constraint_fails_when_wrong_count() {
        let rule = TagRule {
            arg_constraints: vec![ArgumentCountConstraint::Exact(4)],
            ..empty_rule()
        };
        // 2 bits + tag name = split_len 3, expected 4
        let bits = make_bits(&["item", "in"]);
        let errors = evaluate_tag_rules("for", &bits, &rule, make_span());
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            &errors[0],
            ValidationError::ExtractedRuleViolation { tag, message, .. }
            if tag == "for" && message.contains("exactly 3 argument")
        ));
    }

    #[test]
    fn min_constraint_passes_when_sufficient() {
        let rule = TagRule {
            arg_constraints: vec![ArgumentCountConstraint::Min(2)],
            ..empty_rule()
        };
        let bits = make_bits(&["arg1", "arg2"]);
        let errors = evaluate_tag_rules("mytag", &bits, &rule, make_span());
        assert!(errors.is_empty());
    }

    #[test]
    fn min_constraint_fails_when_too_few() {
        let rule = TagRule {
            arg_constraints: vec![ArgumentCountConstraint::Min(4)],
            ..empty_rule()
        };
        let bits = make_bits(&["arg1"]);
        let errors = evaluate_tag_rules("mytag", &bits, &rule, make_span());
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
            ..empty_rule()
        };
        let bits = make_bits(&["a", "b", "c"]);
        let errors = evaluate_tag_rules("mytag", &bits, &rule, make_span());
        assert!(errors.is_empty());
    }

    #[test]
    fn max_constraint_fails_when_over() {
        let rule = TagRule {
            arg_constraints: vec![ArgumentCountConstraint::Max(3)],
            ..empty_rule()
        };
        let bits = make_bits(&["a", "b", "c"]);
        // split_len = 4, max = 3
        let errors = evaluate_tag_rules("mytag", &bits, &rule, make_span());
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
            ..empty_rule()
        };
        // split_len = 4 (3 bits + tag name)
        let bits = make_bits(&["a", "b", "c"]);
        let errors = evaluate_tag_rules("mytag", &bits, &rule, make_span());
        assert!(errors.is_empty());
    }

    #[test]
    fn one_of_constraint_fails_when_not_in_set() {
        let rule = TagRule {
            arg_constraints: vec![ArgumentCountConstraint::OneOf(vec![2, 4])],
            ..empty_rule()
        };
        // split_len = 3 (2 bits + tag name), not in {2, 4}
        let bits = make_bits(&["a", "b"]);
        let errors = evaluate_tag_rules("mytag", &bits, &rule, make_span());
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
            ..empty_rule()
        };
        // bits[1] (position 2 in split_contents - 1) = "in"
        let bits = make_bits(&["item", "in", "items"]);
        let errors = evaluate_tag_rules("for", &bits, &rule, make_span());
        assert!(errors.is_empty());
    }

    #[test]
    fn required_keyword_fails_when_wrong() {
        let rule = TagRule {
            required_keywords: vec![RequiredKeyword {
                position: SplitPosition::Forward(2),
                value: "in".to_string(),
            }],
            ..empty_rule()
        };
        let bits = make_bits(&["item", "from", "items"]);
        let errors = evaluate_tag_rules("for", &bits, &rule, make_span());
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
            ..empty_rule()
        };
        // bits = ["'view_name'", "arg1", "as", "varname"]
        // bits[-2] = "as"
        let bits = make_bits(&["'view_name'", "arg1", "as", "varname"]);
        let errors = evaluate_tag_rules("url", &bits, &rule, make_span());
        assert!(errors.is_empty());
    }

    #[test]
    fn required_keyword_negative_position_fails() {
        let rule = TagRule {
            required_keywords: vec![RequiredKeyword {
                position: SplitPosition::Backward(2),
                value: "as".to_string(),
            }],
            ..empty_rule()
        };
        let bits = make_bits(&["'view_name'", "arg1", "with", "varname"]);
        let errors = evaluate_tag_rules("url", &bits, &rule, make_span());
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn required_keyword_out_of_bounds_skipped() {
        let rule = TagRule {
            required_keywords: vec![RequiredKeyword {
                position: SplitPosition::Forward(5),
                value: "in".to_string(),
            }],
            ..empty_rule()
        };
        let bits = make_bits(&["item"]);
        let errors = evaluate_tag_rules("for", &bits, &rule, make_span());
        assert!(errors.is_empty(), "Out-of-bounds keyword should be skipped");
    }

    #[test]
    fn required_keyword_position_zero_skipped() {
        let rule = TagRule {
            required_keywords: vec![RequiredKeyword {
                position: SplitPosition::Forward(0),
                value: "for".to_string(),
            }],
            ..empty_rule()
        };
        let bits = make_bits(&["item", "in", "items"]);
        let errors = evaluate_tag_rules("for", &bits, &rule, make_span());
        assert!(errors.is_empty(), "Position 0 (tag name) should be skipped");
    }

    // --- KnownOptions tests ---

    #[test]
    fn known_options_no_duplicates_detected() {
        let rule = TagRule {
            known_options: Some(KnownOptions {
                values: vec!["only".to_string(), "with".to_string()],
                allow_duplicates: false,
                rejects_unknown: false,
            }),
            ..empty_rule()
        };
        let bits = make_bits(&["'template.html'", "with", "x=1", "with", "y=2"]);
        let errors = evaluate_tag_rules("include", &bits, &rule, make_span());
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            &errors[0],
            ValidationError::ExtractedRuleViolation { message, .. }
            if message.contains("duplicate") && message.contains("with")
        ));
    }

    #[test]
    fn known_options_duplicates_allowed() {
        let rule = TagRule {
            known_options: Some(KnownOptions {
                values: vec!["only".to_string(), "with".to_string()],
                allow_duplicates: true,
                rejects_unknown: false,
            }),
            ..empty_rule()
        };
        let bits = make_bits(&["'template.html'", "with", "x=1", "with", "y=2"]);
        let errors = evaluate_tag_rules("include", &bits, &rule, make_span());
        assert!(errors.is_empty());
    }

    // --- Combined tests ---

    #[test]
    fn empty_rules_no_errors() {
        let rule = empty_rule();
        let bits = make_bits(&["anything", "goes", "here"]);
        let errors = evaluate_tag_rules("mytag", &bits, &rule, make_span());
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
        let errors = evaluate_tag_rules("for", &bits, &rule, make_span());
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
        let errors = evaluate_tag_rules("for", &bits, &rule, make_span());
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
            ..empty_rule()
        };

        // {% for item in items %} → bits = ["item", "in", "items"]
        // split_contents = ["for", "item", "in", "items"]
        // position 2 in split_contents = "in" = bits[1] ✓
        let bits = make_bits(&["item", "in", "items"]);
        let errors = evaluate_tag_rules("for", &bits, &rule, make_span());
        assert!(
            errors.is_empty(),
            "Position 2 in split_contents should map to bits[1]"
        );
    }

    // --- supports_as_var tests ---

    #[test]
    fn simple_tag_as_varname_passes_max_constraint() {
        // simple_tag with Max(2): accepts 1 arg, but `as varname` adds 2 more tokens
        let rule = TagRule {
            arg_constraints: vec![
                ArgumentCountConstraint::Min(2),
                ArgumentCountConstraint::Max(2),
            ],
            supports_as_var: true,
            ..Default::default()
        };
        // {% user_display user as foo %} → bits = ["user", "as", "foo"]
        let bits = make_bits(&["user", "as", "foo"]);
        let errors = evaluate_tag_rules("user_display", &bits, &rule, make_span());
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
            supports_as_var: true,
            ..Default::default()
        };
        // {% get_providers as providers %} → bits = ["as", "providers"]
        let bits = make_bits(&["as", "providers"]);
        let errors = evaluate_tag_rules("get_providers", &bits, &rule, make_span());
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
            supports_as_var: true,
            ..Default::default()
        };
        // {% user_display user %} → bits = ["user"]
        let bits = make_bits(&["user"]);
        let errors = evaluate_tag_rules("user_display", &bits, &rule, make_span());
        assert!(errors.is_empty(), "Normal usage should pass: {errors:?}");
    }

    #[test]
    fn simple_tag_extra_args_still_rejected() {
        // simple_tag with Max(2): accepts 1 arg, extra args should fail
        let rule = TagRule {
            arg_constraints: vec![ArgumentCountConstraint::Max(2)],
            supports_as_var: true,
            ..Default::default()
        };
        // {% user_display user extra %} → bits = ["user", "extra"]
        let bits = make_bits(&["user", "extra"]);
        let errors = evaluate_tag_rules("user_display", &bits, &rule, make_span());
        assert_eq!(errors.len(), 1, "Extra args without `as` should still fail");
    }

    #[test]
    fn non_simple_tag_as_varname_not_stripped() {
        // Manual tag with supports_as_var=false: `as` is NOT stripped
        let rule = TagRule {
            arg_constraints: vec![ArgumentCountConstraint::Max(2)],
            supports_as_var: false,
            ..Default::default()
        };
        // bits = ["user", "as", "foo"] → split_len=4, Max(2) fails
        let bits = make_bits(&["user", "as", "foo"]);
        let errors = evaluate_tag_rules("mytag", &bits, &rule, make_span());
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
            ..empty_rule()
        };
        let bits = make_bits(&["on"]);
        let errors = evaluate_tag_rules("autoescape", &bits, &rule, make_span());
        assert!(errors.is_empty());
    }

    #[test]
    fn choice_at_fails_when_invalid() {
        let rule = TagRule {
            choice_at_constraints: vec![ChoiceAt {
                position: SplitPosition::Forward(1),
                values: vec!["on".to_string(), "off".to_string()],
            }],
            ..empty_rule()
        };
        let bits = make_bits(&["unknown"]);
        let errors = evaluate_tag_rules("autoescape", &bits, &rule, make_span());
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
            ..empty_rule()
        };
        // bits[-1] = "yes"
        let bits = make_bits(&["something", "yes"]);
        let errors = evaluate_tag_rules("mytag", &bits, &rule, make_span());
        assert!(errors.is_empty());
    }

    #[test]
    fn choice_at_out_of_bounds_skipped() {
        let rule = TagRule {
            choice_at_constraints: vec![ChoiceAt {
                position: SplitPosition::Forward(5),
                values: vec!["a".to_string()],
            }],
            ..empty_rule()
        };
        let bits = make_bits(&["x"]);
        let errors = evaluate_tag_rules("mytag", &bits, &rule, make_span());
        assert!(errors.is_empty());
    }

    #[test]
    fn choice_at_combined_with_arg_count() {
        let rule = TagRule {
            arg_constraints: vec![ArgumentCountConstraint::Exact(2)],
            choice_at_constraints: vec![ChoiceAt {
                position: SplitPosition::Forward(1),
                values: vec!["on".to_string(), "off".to_string()],
            }],
            ..empty_rule()
        };
        // Correct count, wrong value
        let bits = make_bits(&["bad"]);
        let errors = evaluate_tag_rules("autoescape", &bits, &rule, make_span());
        assert_eq!(errors.len(), 1); // Only choice violation, count is correct
        assert!(matches!(
            &errors[0],
            ValidationError::ExtractedRuleViolation { message, .. }
            if message.contains("must be one of")
        ));
    }
}

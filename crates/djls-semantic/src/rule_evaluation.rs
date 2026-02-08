use djls_extraction::ArgumentCountConstraint;
use djls_extraction::KnownOptions;
use djls_extraction::RequiredKeyword;
use djls_extraction::TagRule;
use djls_source::Span;

use crate::ValidationError;

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

    // split_contents() length = bits.len() + 1 (tag name at index 0)
    let split_len = effective_bits.len() + 1;

    for constraint in &rules.arg_constraints {
        if let Some(error) = evaluate_arg_constraint(tag_name, split_len, constraint, span) {
            errors.push(error);
        }
    }

    for keyword in &rules.required_keywords {
        if let Some(error) = evaluate_required_keyword(tag_name, effective_bits, keyword, span) {
            errors.push(error);
        }
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

/// Evaluate an argument count constraint.
///
/// Constraints express the conditions from Django source that raise
/// `TemplateSyntaxError`. The extraction captures what makes the tag **valid**:
/// - `Exact(N)`: valid when `split_len == N`
/// - `Min(N)`: valid when `split_len >= N`
/// - `Max(N)`: valid when `split_len <= N`
/// - `OneOf(set)`: valid when `split_len in set`
fn evaluate_arg_constraint(
    tag_name: &str,
    split_len: usize,
    constraint: &ArgumentCountConstraint,
    span: Span,
) -> Option<ValidationError> {
    let violated = match constraint {
        ArgumentCountConstraint::Exact(n) => split_len != *n,
        ArgumentCountConstraint::Min(n) => split_len < *n,
        ArgumentCountConstraint::Max(n) => split_len > *n,
        ArgumentCountConstraint::OneOf(values) => !values.contains(&split_len),
    };

    if violated {
        let message = match constraint {
            ArgumentCountConstraint::Exact(n) => {
                // n includes tag name, so actual arg count = n - 1
                let expected_args = n - 1;
                let actual_args = split_len - 1;
                format!(
                    "'{tag_name}' takes exactly {expected_args} argument{}, {actual_args} given",
                    if expected_args == 1 { "" } else { "s" }
                )
            }
            ArgumentCountConstraint::Min(n) => {
                let min_args = n - 1;
                format!(
                    "'{tag_name}' requires at least {min_args} argument{}",
                    if min_args == 1 { "" } else { "s" }
                )
            }
            ArgumentCountConstraint::Max(n) => {
                let max_args = n - 1;
                format!(
                    "'{tag_name}' accepts at most {max_args} argument{}",
                    if max_args == 1 { "" } else { "s" }
                )
            }
            ArgumentCountConstraint::OneOf(values) => {
                let arg_counts: Vec<String> = values.iter().map(|v| (v - 1).to_string()).collect();
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

/// Evaluate a required keyword constraint.
///
/// `RequiredKeyword.position` uses `split_contents()` indexing (tag name at 0).
/// Positive positions index from the start, negative from the end.
/// If the position is out of bounds (tag too short), we skip — the argument
/// count constraint should catch that case.
fn evaluate_required_keyword(
    tag_name: &str,
    bits: &[String],
    keyword: &RequiredKeyword,
    span: Span,
) -> Option<ValidationError> {
    let bits_index = if keyword.position >= 0 {
        // Adjust from split_contents index to bits index (subtract 1 for tag name)
        let Ok(idx) = usize::try_from(keyword.position) else {
            return None;
        };
        if idx == 0 {
            return None; // Position 0 is the tag name itself, skip
        }
        idx - 1
    } else {
        // Negative indexing from end — maps directly since the end is the same
        let Ok(abs_pos) = usize::try_from(keyword.position.unsigned_abs()) else {
            return None;
        };
        if abs_pos > bits.len() {
            return None; // Out of bounds, skip
        }
        bits.len() - abs_pos
    };

    if bits_index >= bits.len() {
        return None; // Out of bounds — arg count constraint should catch this
    }

    if bits[bits_index] == keyword.value {
        None
    } else {
        Some(ValidationError::ExtractedRuleViolation {
            tag: tag_name.to_string(),
            message: format!(
                "'{tag_name}' expected '{}' at position {}",
                keyword.value, keyword.position
            ),
            span,
        })
    }
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
                position: 2,
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
                position: 2,
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
                position: -2,
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
                position: -2,
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
                position: 5,
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
                position: 0,
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
                position: 2,
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
                position: 2,
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
                position: 2,
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
}

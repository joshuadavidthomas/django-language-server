//! Evaluate extracted rules against template tag arguments.
//!
//! This module provides the evaluation engine that validates template tag
//! arguments against [`ExtractedRule`] conditions mined from Python source.
//!
//! # Index Offset
//!
//! Extraction rules use `split_contents()` indices where the tag name is at
//! index 0. The template parser's `bits` EXCLUDES the tag name. The evaluator
//! adjusts: extraction `index N` → `bits[N-1]`.
//!
//! Example: extraction says `LiteralAt{index:2, value:"in"}` for the `for` tag.
//! In bits (which has `["item", "in", "items"]`), "in" is at index 1, not 2.
//!
//! # Negation Semantics
//!
//! Many conditions have a `negated` flag. The rule represents the ERROR condition:
//! - `ExactArgCount{count:2, negated:true}` → error when `len(bits)+1 != 2`
//!   (i.e., the tag should have exactly 2 words including tag name)
//! - `LiteralAt{index:2, value:"in", negated:true}` → error when `bits[1] != "in"`
//!
//! # Opaque Rules
//!
//! [`RuleCondition::Opaque`] means extraction couldn't simplify the condition.
//! These are silently skipped — no validation, not treated as errors.
//!
//! # Error Messages
//!
//! [`ExtractedRule::message`] contains the original Django error message (e.g.,
//! "'for' statements should have at least four words"). When available, this
//! is used directly in the diagnostic. When absent, a generic message is
//! constructed from the condition.

use djls_extraction::{ExtractedRule, RuleCondition};
use djls_source::Span;
use salsa::Accumulator;

use crate::{Db, ValidationError, ValidationErrorAccumulator};

/// Evaluate extracted rules against template tag arguments.
///
/// Iterates through all rules and accumulates errors for any violations.
/// Rules are evaluated independently — multiple violations can be reported.
///
/// # Arguments
/// - `db`: The Salsa database for accumulating errors
/// - `tag_name`: The name of the tag being validated
/// - `bits`: The tokenized arguments (excluding tag name)
/// - `rules`: The extracted rules from Python source
/// - `span`: The span for error reporting
pub fn evaluate_extracted_rules(
    db: &dyn Db,
    tag_name: &str,
    bits: &[String],
    rules: &[ExtractedRule],
    span: Span,
) {
    // bits length in split_contents terms (includes tag name at index 0)
    let split_len = bits.len() + 1;

    for rule in rules {
        let violated = evaluate_condition(&rule.condition, bits, split_len);

        if violated {
            let error = create_error(tag_name, bits, rule, span);
            ValidationErrorAccumulator(error).accumulate(db);
        }
    }
}

/// Evaluate a single condition against the tag arguments.
///
/// Returns `true` if the condition is violated (error should be reported).
fn evaluate_condition(condition: &RuleCondition, bits: &[String], split_len: usize) -> bool {
    match condition {
        RuleCondition::ExactArgCount { count, negated } => {
            let matches = split_len == *count;
            if *negated {
                !matches
            } else {
                matches
            }
        }

        RuleCondition::ArgCountComparison { count, op } => {
            use djls_extraction::ComparisonOp;
            match op {
                ComparisonOp::Lt => split_len < *count,
                ComparisonOp::LtEq => split_len <= *count,
                ComparisonOp::Gt => split_len > *count,
                ComparisonOp::GtEq => split_len >= *count,
            }
        }

        RuleCondition::MinArgCount { min } => split_len < *min,

        RuleCondition::MaxArgCount { max } => split_len <= *max,

        RuleCondition::LiteralAt { index, value, negated } => {
            // Adjust for index offset: extraction index N → bits[N-1]
            let bits_index = index.saturating_sub(1);
            let matches = bits.get(bits_index) == Some(value);
            if *negated {
                !matches
            } else {
                matches
            }
        }

        RuleCondition::ChoiceAt { index, choices, negated } => {
            let bits_index = index.saturating_sub(1);
            let matches = bits
                .get(bits_index)
                .is_some_and(|bit| choices.iter().any(|choice| choice == bit));
            if *negated {
                !matches
            } else {
                matches
            }
        }

        RuleCondition::ContainsLiteral { value, negated } => {
            let contains = bits.iter().any(|bit| bit == value);
            if *negated {
                !contains
            } else {
                contains
            }
        }

        RuleCondition::Opaque { .. } => {
            // Can't evaluate — skip silently (not a violation)
            false
        }
    }
}

/// Create a validation error for a violated rule.
fn create_error(
    tag_name: &str,
    bits: &[String],
    rule: &ExtractedRule,
    span: Span,
) -> ValidationError {
    // Use the original Django error message if available
    let message = rule
        .message
        .clone()
        .unwrap_or_else(|| format_default_message(tag_name, bits, &rule.condition));

    ValidationError::ExtractedRuleViolation {
        tag: tag_name.to_string(),
        message,
        span,
    }
}

/// Generate a default error message when extraction didn't provide one.
fn format_default_message(tag_name: &str, _bits: &[String], condition: &RuleCondition) -> String {
    match condition {
        RuleCondition::ExactArgCount { count, negated } => {
            if *negated {
                format!("'{tag_name}' tag should have exactly {count} words")
            } else {
                format!("'{tag_name}' tag has exactly {count} words (unexpected)")
            }
        }
        RuleCondition::MinArgCount { min } => {
            format!("'{tag_name}' tag should have at least {min} words")
        }
        RuleCondition::MaxArgCount { max } => {
            // Note: MaxArgCount semantics are inverted - it represents "at least" via threshold
            format!("'{tag_name}' tag should have more than {max} words")
        }
        RuleCondition::ArgCountComparison { count, op } => {
            use djls_extraction::ComparisonOp;
            let op_str = match op {
                ComparisonOp::Lt => "less than",
                ComparisonOp::LtEq => "at most",
                ComparisonOp::Gt => "more than",
                ComparisonOp::GtEq => "at least",
            };
            format!("'{tag_name}' tag should have {op_str} {count} words")
        }
        RuleCondition::LiteralAt { index, value, negated } => {
            let bits_index = index.saturating_sub(1);
            if *negated {
                format!("'{tag_name}' tag word {bits_index} should be '{value}'")
            } else {
                format!("'{tag_name}' tag word {bits_index} should not be '{value}'")
            }
        }
        RuleCondition::ChoiceAt { index, choices, negated } => {
            let bits_index = index.saturating_sub(1);
            let choices_str = choices.join("', '");
            if *negated {
                format!(
                    "'{tag_name}' tag word {bits_index} should be one of: '{choices_str}'"
                )
            } else {
                format!(
                    "'{tag_name}' tag word {bits_index} should not be one of: '{choices_str}'"
                )
            }
        }
        RuleCondition::ContainsLiteral { value, negated } => {
            if *negated {
                format!("'{tag_name}' tag should contain '{value}'")
            } else {
                format!("'{tag_name}' tag should not contain '{value}'")
            }
        }
        RuleCondition::Opaque { description } => {
            format!("'{tag_name}' tag argument validation failed: {description}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use djls_extraction::{ComparisonOp, RuleCondition};



    #[test]
    fn test_exact_arg_count_negated_true() {
        // Rule: error when split_len != 2
        let condition = RuleCondition::ExactArgCount {
            count: 2,
            negated: true,
        };

        // split_len = 3 (bits len 2 + 1), should violate (3 != 2)
        let bits = vec!["arg1".to_string(), "arg2".to_string()];
        assert!(evaluate_condition(&condition, &bits, 3));

        // split_len = 2 (bits len 1 + 1), should NOT violate (2 == 2)
        let bits = vec!["arg1".to_string()];
        assert!(!evaluate_condition(&condition, &bits, 2));
    }

    #[test]
    fn test_exact_arg_count_negated_false() {
        // Rule: error when split_len == 2
        let condition = RuleCondition::ExactArgCount {
            count: 2,
            negated: false,
        };

        // split_len = 2 (bits len 1 + 1), should violate (2 == 2)
        let bits = vec!["arg1".to_string()];
        assert!(evaluate_condition(&condition, &bits, 2));

        // split_len = 3 (bits len 2 + 1), should NOT violate (3 != 2)
        let bits = vec!["arg1".to_string(), "arg2".to_string()];
        assert!(!evaluate_condition(&condition, &bits, 3));
    }

    #[test]
    fn test_max_arg_count() {
        // Rule: MaxArgCount{max:3} = error when split_len <= 3
        // (For 'for' tag: "'for' statements should have at least four words")
        let condition = RuleCondition::MaxArgCount { max: 3 };

        // split_len = 3, should violate (3 <= 3)
        let bits = vec!["item".to_string(), "in".to_string()];
        assert!(evaluate_condition(&condition, &bits, 3));

        // split_len = 4, should NOT violate (4 > 3)
        let bits = vec!["item".to_string(), "in".to_string(), "items".to_string()];
        assert!(!evaluate_condition(&condition, &bits, 4));
    }

    #[test]
    fn test_min_arg_count() {
        // Rule: MinArgCount{min:4} = error when split_len < 4
        let condition = RuleCondition::MinArgCount { min: 4 };

        // split_len = 3, should violate (3 < 4)
        let bits = vec!["item".to_string(), "in".to_string()];
        assert!(evaluate_condition(&condition, &bits, 3));

        // split_len = 4, should NOT violate (4 >= 4)
        let bits = vec!["item".to_string(), "in".to_string(), "items".to_string()];
        assert!(!evaluate_condition(&condition, &bits, 4));
    }

    #[test]
    fn test_arg_count_comparison() {
        // Rule: ArgCountComparison{count:5, op:Gt} = error when split_len > 5
        let condition = RuleCondition::ArgCountComparison {
            count: 5,
            op: ComparisonOp::Gt,
        };

        // split_len = 6, should violate (6 > 5)
        let bits = vec!["a".to_string(), "b".to_string(), "c".to_string(), "d".to_string(), "e".to_string()];
        assert!(evaluate_condition(&condition, &bits, 6));

        // split_len = 5, should NOT violate (5 > 5 is false)
        let bits = vec!["a".to_string(), "b".to_string(), "c".to_string(), "d".to_string()];
        assert!(!evaluate_condition(&condition, &bits, 5));
    }

    #[test]
    fn test_literal_at_negated_true() {
        // Rule: LiteralAt{index:2, value:"in", negated:true}
        // = error when bits[1] != "in"
        let condition = RuleCondition::LiteralAt {
            index: 2,
            value: "in".to_string(),
            negated: true,
        };

        // bits[1] = "not_in", should violate
        let bits = vec!["item".to_string(), "not_in".to_string(), "items".to_string()];
        assert!(evaluate_condition(&condition, &bits, 4));

        // bits[1] = "in", should NOT violate
        let bits = vec!["item".to_string(), "in".to_string(), "items".to_string()];
        assert!(!evaluate_condition(&condition, &bits, 4));
    }

    #[test]
    fn test_literal_at_negated_false() {
        // Rule: LiteralAt{index:2, value:"bad", negated:false}
        // = error when bits[1] == "bad"
        let condition = RuleCondition::LiteralAt {
            index: 2,
            value: "bad".to_string(),
            negated: false,
        };

        // bits[1] = "bad", should violate
        let bits = vec!["item".to_string(), "bad".to_string(), "items".to_string()];
        assert!(evaluate_condition(&condition, &bits, 4));

        // bits[1] = "good", should NOT violate
        let bits = vec!["item".to_string(), "good".to_string(), "items".to_string()];
        assert!(!evaluate_condition(&condition, &bits, 4));
    }

    #[test]
    fn test_literal_at_index_out_of_bounds() {
        // Rule: LiteralAt{index:10, value:"test", negated:true}
        // bits only has 3 elements, so bits[9] doesn't exist
        let condition = RuleCondition::LiteralAt {
            index: 10,
            value: "test".to_string(),
            negated: true,
        };

        let bits = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        // bits_index = 9, which is out of bounds, so matches is false
        // negated=true means violated = !false = true
        assert!(evaluate_condition(&condition, &bits, 4));
    }

    #[test]
    fn test_choice_at_negated_true() {
        // Rule: ChoiceAt{index:1, choices:["on","off"], negated:true}
        // = error when bits[0] not in choices
        let condition = RuleCondition::ChoiceAt {
            index: 1,
            choices: vec!["on".to_string(), "off".to_string()],
            negated: true,
        };

        // bits[0] = "invalid", should violate
        let bits = vec!["invalid".to_string()];
        assert!(evaluate_condition(&condition, &bits, 2));

        // bits[0] = "on", should NOT violate
        let bits = vec!["on".to_string()];
        assert!(!evaluate_condition(&condition, &bits, 2));
    }

    #[test]
    fn test_choice_at_negated_false() {
        // Rule: ChoiceAt{index:1, choices:["bad"], negated:false}
        // = error when bits[0] in choices
        let condition = RuleCondition::ChoiceAt {
            index: 1,
            choices: vec!["bad".to_string()],
            negated: false,
        };

        // bits[0] = "bad", should violate
        let bits = vec!["bad".to_string()];
        assert!(evaluate_condition(&condition, &bits, 2));

        // bits[0] = "good", should NOT violate
        let bits = vec!["good".to_string()];
        assert!(!evaluate_condition(&condition, &bits, 2));
    }

    #[test]
    fn test_contains_literal_negated_true() {
        // Rule: ContainsLiteral{value:"as", negated:true}
        // = error when "as" not in bits
        let condition = RuleCondition::ContainsLiteral {
            value: "as".to_string(),
            negated: true,
        };

        // "as" not in bits, should violate
        let bits = vec!["item".to_string(), "in".to_string(), "items".to_string()];
        assert!(evaluate_condition(&condition, &bits, 4));

        // "as" in bits, should NOT violate
        let bits = vec!["item".to_string(), "as".to_string(), "var".to_string()];
        assert!(!evaluate_condition(&condition, &bits, 4));
    }

    #[test]
    fn test_contains_literal_negated_false() {
        // Rule: ContainsLiteral{value:"bad", negated:false}
        // = error when "bad" in bits
        let condition = RuleCondition::ContainsLiteral {
            value: "bad".to_string(),
            negated: false,
        };

        // "bad" in bits, should violate
        let bits = vec!["item".to_string(), "bad".to_string(), "items".to_string()];
        assert!(evaluate_condition(&condition, &bits, 4));

        // "bad" not in bits, should NOT violate
        let bits = vec!["item".to_string(), "in".to_string(), "items".to_string()];
        assert!(!evaluate_condition(&condition, &bits, 4));
    }

    #[test]
    fn test_opaque_always_skipped() {
        // Rule: Opaque = always skipped, never violated
        let condition = RuleCondition::Opaque {
            description: "complex condition".to_string(),
        };

        let bits = vec!["anything".to_string()];
        assert!(!evaluate_condition(&condition, &bits, 2));
    }

    #[test]
    fn test_index_offset_for_tag_name() {
        // For extraction index 0 (tag name), bits_index = 0 - 1 = 0
        // This is a special case - the tag name is always at extraction index 0
        // but we don't validate it (it's always correct by definition)
        //
        // However, if someone writes a rule for index 0, we should handle it gracefully
        let condition = RuleCondition::LiteralAt {
            index: 0,
            value: "tagname".to_string(),
            negated: true,
        };

        // bits_index = 0.saturating_sub(1) = 0
        // We check bits[0] against "tagname"
        let bits = vec!["not_tagname".to_string(), "arg".to_string()];
        // bits[0] = "not_tagname" != "tagname", so matches = false
        // negated = true, so violated = !false = true
        assert!(evaluate_condition(&condition, &bits, 3));
    }

    #[test]
    fn test_empty_rules() {
        // Empty rules should produce no errors
        let rules: Vec<ExtractedRule> = vec![];
        let bits: Vec<String> = vec!["any".to_string()];

        for rule in &rules {
            let violated = evaluate_condition(&rule.condition, &bits, bits.len() + 1);
            assert!(!violated);
        }
    }

    #[test]
    fn test_default_message_generation() {
        // Test that default messages are generated correctly
        let condition = RuleCondition::ExactArgCount {
            count: 2,
            negated: true,
        };
        let msg = format_default_message("test", &[], &condition);
        assert_eq!(msg, "'test' tag should have exactly 2 words");

        let condition = RuleCondition::MaxArgCount { max: 3 };
        let msg = format_default_message("for", &[], &condition);
        assert_eq!(msg, "'for' tag should have more than 3 words");

        let condition = RuleCondition::LiteralAt {
            index: 2,
            value: "in".to_string(),
            negated: true,
        };
        let msg = format_default_message("for", &[], &condition);
        assert_eq!(msg, "'for' tag word 1 should be 'in'");
    }
}

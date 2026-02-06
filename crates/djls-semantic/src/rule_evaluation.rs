//! Extracted rule evaluation for template tag argument validation.
//!
//! Evaluates `ExtractedRule` conditions against template tag arguments,
//! replacing the old hand-crafted `TagArg`-based validation path.
//!
//! # Index Offset
//!
//! Extraction rules use `split_contents()` indices where the tag name is at
//! index 0. The template parser's `bits` EXCLUDES the tag name. The evaluator
//! adjusts: extraction `index N` → `bits[N-1]`.
//!
//! # Negation Semantics
//!
//! Rules represent the ERROR condition — the rule fires (produces an error)
//! when the condition evaluates to true:
//! - `ExactArgCount{count:2, negated:true}` → error when `len(bits)+1 != 2`
//! - `LiteralAt{index:2, value:"in", negated:true}` → error when `bits[1] != "in"`
//!
//! # Opaque Rules
//!
//! `RuleCondition::Opaque` means extraction couldn't simplify the condition.
//! These are silently skipped — no validation, not treated as errors.

use djls_extraction::ComparisonOp;
use djls_extraction::ExtractedRule;
use djls_extraction::RuleCondition;
use djls_source::Span;
use salsa::Accumulator;

use crate::db::ValidationErrorAccumulator;
use crate::errors::ValidationError;

/// Evaluate extracted rules against template tag arguments.
///
/// All rules are evaluated independently — a violation in one rule does
/// not short-circuit evaluation of the remaining rules.
pub fn evaluate_extracted_rules(
    db: &dyn crate::Db,
    tag_name: &str,
    bits: &[String],
    rules: &[ExtractedRule],
    span: Span,
) {
    // split_contents() length includes the tag name
    let split_len = bits.len() + 1;

    for rule in rules {
        let violated = evaluate_condition(&rule.condition, bits, split_len);

        if violated {
            let message = rule.message.clone().unwrap_or_else(|| {
                format!("Tag '{tag_name}' argument violation")
            });

            ValidationErrorAccumulator(ValidationError::ExtractedRuleViolation {
                tag: tag_name.to_string(),
                message,
                span,
            })
            .accumulate(db);
        }
    }
}

/// Evaluate a single rule condition against the given bits.
///
/// Returns `true` if the condition is violated (error should be emitted).
fn evaluate_condition(
    condition: &RuleCondition,
    bits: &[String],
    split_len: usize,
) -> bool {
    match condition {
        RuleCondition::ExactArgCount { count, negated } => {
            let matches = split_len == *count;
            if *negated { !matches } else { matches }
        }

        RuleCondition::ArgCountComparison { count, op } => match op {
            ComparisonOp::Lt => split_len < *count,
            ComparisonOp::LtEq => split_len <= *count,
            ComparisonOp::Gt => split_len > *count,
            ComparisonOp::GtEq => split_len >= *count,
        },

        RuleCondition::MinArgCount { min } => split_len < *min,

        RuleCondition::MaxArgCount { max } => split_len <= *max,

        RuleCondition::LiteralAt {
            index,
            value,
            negated,
        } => {
            let Some(bits_index) = index.checked_sub(1) else {
                // index 0 is tag name, always correct
                return false;
            };
            let matches = bits.get(bits_index) == Some(value);
            if *negated { !matches } else { matches }
        }

        RuleCondition::ChoiceAt {
            index,
            choices,
            negated,
        } => {
            let Some(bits_index) = index.checked_sub(1) else {
                return false;
            };
            let matches = bits
                .get(bits_index)
                .is_some_and(|b| choices.iter().any(|c| c == b));
            if *negated { !matches } else { matches }
        }

        RuleCondition::ContainsLiteral { value, negated } => {
            let contains = bits.iter().any(|b| b == value);
            if *negated { !contains } else { contains }
        }

        RuleCondition::Opaque { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test helper: evaluate a single condition and return whether it was violated.
    fn is_violated(condition: &RuleCondition, args: &[&str]) -> bool {
        let b: Vec<String> = args.iter().copied().map(String::from).collect();
        let split_len = b.len() + 1;
        evaluate_condition(condition, &b, split_len)
    }

    // --- ExactArgCount tests ---

    #[test]
    fn exact_arg_count_negated_violated_when_not_matching() {
        assert!(is_violated(&
            RuleCondition::ExactArgCount {
                count: 2,
                negated: true,
            },
            &[], // split_len=1, != 2 → violated
        ));
    }

    #[test]
    fn exact_arg_count_negated_not_violated_when_matching() {
        assert!(!is_violated(&
            RuleCondition::ExactArgCount {
                count: 2,
                negated: true,
            },
            &["on"], // split_len=2, == 2, negated → not violated
        ));
    }

    #[test]
    fn exact_arg_count_not_negated_violated_when_matching() {
        assert!(is_violated(&
            RuleCondition::ExactArgCount {
                count: 2,
                negated: false,
            },
            &["arg1"], // split_len=2, == 2 → violated
        ));
    }

    // --- MaxArgCount tests ---

    #[test]
    fn max_arg_count_violated_when_at_threshold() {
        // MaxArgCount{max:3} → violated when split_len <= 3
        assert!(is_violated(&
            RuleCondition::MaxArgCount { max: 3 },
            &["item", "in"], // split_len=3, 3 <= 3 → violated
        ));
    }

    #[test]
    fn max_arg_count_not_violated_when_above_threshold() {
        assert!(!is_violated(&
            RuleCondition::MaxArgCount { max: 3 },
            &["item", "in", "items"], // split_len=4, 4 <= 3 is false
        ));
    }

    // --- MinArgCount tests ---

    #[test]
    fn min_arg_count_violated_when_below() {
        assert!(is_violated(&
            RuleCondition::MinArgCount { min: 4 },
            &["a", "b"], // split_len=3, 3 < 4 → violated
        ));
    }

    #[test]
    fn min_arg_count_not_violated_at_minimum() {
        assert!(!is_violated(&
            RuleCondition::MinArgCount { min: 4 },
            &["a", "b", "c"], // split_len=4, 4 < 4 is false
        ));
    }

    // --- ArgCountComparison tests ---

    #[test]
    fn arg_count_comparison_gt_violated() {
        assert!(is_violated(&
            RuleCondition::ArgCountComparison {
                count: 5,
                op: ComparisonOp::Gt,
            },
            &["a", "b", "c", "d", "e"], // split_len=6, 6 > 5
        ));
    }

    #[test]
    fn arg_count_comparison_gt_not_violated_when_equal() {
        assert!(!is_violated(&
            RuleCondition::ArgCountComparison {
                count: 5,
                op: ComparisonOp::Gt,
            },
            &["a", "b", "c", "d"], // split_len=5, 5 > 5 is false
        ));
    }

    #[test]
    fn arg_count_comparison_lt_violated() {
        assert!(is_violated(&
            RuleCondition::ArgCountComparison {
                count: 3,
                op: ComparisonOp::Lt,
            },
            &["a"], // split_len=2, 2 < 3
        ));
    }

    #[test]
    fn arg_count_comparison_gteq_violated() {
        assert!(is_violated(&
            RuleCondition::ArgCountComparison {
                count: 3,
                op: ComparisonOp::GtEq,
            },
            &["a", "b"], // split_len=3, 3 >= 3
        ));
    }

    #[test]
    fn arg_count_comparison_lteq_violated() {
        assert!(is_violated(&
            RuleCondition::ArgCountComparison {
                count: 3,
                op: ComparisonOp::LtEq,
            },
            &["a", "b"], // split_len=3, 3 <= 3
        ));
    }

    // --- LiteralAt tests ---

    #[test]
    fn literal_at_negated_violated_when_mismatch() {
        // extraction index 2 → bits[1]
        assert!(is_violated(&
            RuleCondition::LiteralAt {
                index: 2,
                value: "in".to_string(),
                negated: true,
            },
            &["item", "nothin", "items"], // bits[1]="nothin" != "in"
        ));
    }

    #[test]
    fn literal_at_negated_not_violated_when_match() {
        assert!(!is_violated(&
            RuleCondition::LiteralAt {
                index: 2,
                value: "in".to_string(),
                negated: true,
            },
            &["item", "in", "items"], // bits[1]="in" → matches, negated
        ));
    }

    #[test]
    fn literal_at_not_negated_violated_when_match() {
        assert!(is_violated(&
            RuleCondition::LiteralAt {
                index: 2,
                value: "bad".to_string(),
                negated: false,
            },
            &["x", "bad"], // bits[1]="bad" → matches
        ));
    }

    #[test]
    fn literal_at_index_zero_never_violated() {
        assert!(!is_violated(&
            RuleCondition::LiteralAt {
                index: 0,
                value: "anything".to_string(),
                negated: true,
            },
            &["arg1"],
        ));
    }

    // --- ChoiceAt tests ---

    #[test]
    fn choice_at_negated_violated_when_not_in_choices() {
        assert!(is_violated(&
            RuleCondition::ChoiceAt {
                index: 1,
                choices: vec!["on".to_string(), "off".to_string()],
                negated: true,
            },
            &["maybe"], // bits[0]="maybe" not in choices
        ));
    }

    #[test]
    fn choice_at_negated_not_violated_when_in_choices() {
        assert!(!is_violated(&
            RuleCondition::ChoiceAt {
                index: 1,
                choices: vec!["on".to_string(), "off".to_string()],
                negated: true,
            },
            &["on"], // bits[0]="on" in choices
        ));
    }

    // --- ContainsLiteral tests ---

    #[test]
    fn contains_literal_negated_violated_when_absent() {
        assert!(is_violated(&
            RuleCondition::ContainsLiteral {
                value: "as".to_string(),
                negated: true,
            },
            &["x", "y"], // "as" not in bits
        ));
    }

    #[test]
    fn contains_literal_negated_not_violated_when_present() {
        assert!(!is_violated(&
            RuleCondition::ContainsLiteral {
                value: "as".to_string(),
                negated: true,
            },
            &["x", "as", "y"],
        ));
    }

    #[test]
    fn contains_literal_not_negated_violated_when_present() {
        assert!(is_violated(&
            RuleCondition::ContainsLiteral {
                value: "forbidden".to_string(),
                negated: false,
            },
            &["a", "forbidden", "b"],
        ));
    }

    // --- Opaque tests ---

    #[test]
    fn opaque_never_violated() {
        assert!(!is_violated(&
            RuleCondition::Opaque {
                description: "unrecognized comparison".to_string(),
            },
            &[],
        ));
    }

    // --- Edge cases ---

    #[test]
    fn out_of_bounds_index_negated_violated() {
        // bits[9] = None → matches=false, negated → violated
        assert!(is_violated(&
            RuleCondition::LiteralAt {
                index: 10,
                value: "missing".to_string(),
                negated: true,
            },
            &["a", "b"],
        ));
    }

    #[test]
    fn out_of_bounds_index_not_negated_not_violated() {
        // bits[9] = None → matches=false, not negated → not violated
        assert!(!is_violated(&
            RuleCondition::LiteralAt {
                index: 10,
                value: "missing".to_string(),
                negated: false,
            },
            &["a", "b"],
        ));
    }

    #[test]
    fn empty_bits_do_not_panic() {
        let b: Vec<String> = vec![];
        assert!(!evaluate_condition(
            &RuleCondition::LiteralAt {
                index: 1,
                value: "x".to_string(),
                negated: false,
            },
            &b,
            1,
        ));
    }

    #[test]
    fn multiple_conditions_independently_evaluated() {
        let b: Vec<String> = vec!["item".to_string()];
        let split_len = b.len() + 1; // 2

        // MaxArgCount{max:3}: 2 <= 3 → violated
        assert!(evaluate_condition(
            &RuleCondition::MaxArgCount { max: 3 },
            &b,
            split_len,
        ));

        // LiteralAt{index:2, negated:true}: bits[1] OOB → false, negated → violated
        assert!(evaluate_condition(
            &RuleCondition::LiteralAt {
                index: 2,
                value: "in".to_string(),
                negated: true,
            },
            &b,
            split_len,
        ));
    }
}

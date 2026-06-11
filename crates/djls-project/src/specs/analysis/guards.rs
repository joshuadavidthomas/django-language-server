//! If/raise guard extraction.
//!
//! Finds `if condition: raise SomeException(...)` patterns and interprets
//! the condition against the abstract environment to produce rule fragments.
//!
//! Any uncaught raise in an if-body is treated as a validation constraint,
//! regardless of exception type (e.g., `TemplateSyntaxError`, `ValueError`).

use ruff_python_ast::BoolOp;
use ruff_python_ast::CmpOp;
use ruff_python_ast::Expr;
use ruff_python_ast::ExprBoolOp;
use ruff_python_ast::ExprCompare;
use ruff_python_ast::ExprUnaryOp;
use ruff_python_ast::StmtIf;
use ruff_python_ast::UnaryOp;

use crate::extraction::ext::ExprExt;
use crate::specs::analysis::constraints::ExtractedTagConstraints;
use crate::specs::analysis::exceptions::direct_raise_exception;
use crate::specs::analysis::exceptions::extract_exception_message;
use crate::specs::analysis::expressions::eval_expr;
use crate::specs::analysis::state::AbstractValue;
use crate::specs::analysis::state::Env;
use crate::specs::types::ArgumentCountConstraint;
use crate::specs::types::ChoiceAt;
use crate::specs::types::ExtractedDiagnosticConstraint;
use crate::specs::types::ExtractedDiagnosticMessage;
use crate::specs::types::RequiredKeyword;

/// Rule fragments contributed by one or more raising guards.
#[derive(Debug, Clone, Default)]
pub(crate) struct ExtractedRuleFragment {
    pub constraints: ExtractedTagConstraints,
    pub diagnostic_messages: Vec<ExtractedDiagnosticMessage>,
}

impl ExtractedRuleFragment {
    pub(crate) fn extend(&mut self, other: Self) {
        self.constraints.extend(other.constraints);
        self.diagnostic_messages.extend(other.diagnostic_messages);
    }
}

/// Extract rule fragments from a single if-statement using the current env state.
///
/// Called inline during statement processing so that constraints see the env
/// as it exists at the point in the code where the if-statement appears,
/// not the final env state after the entire function body has been processed.
pub(crate) fn extract_from_if_inline(if_stmt: &StmtIf, env: &mut Env) -> ExtractedRuleFragment {
    let mut result = ExtractedRuleFragment::default();

    if let Some(raised_exception) = direct_raise_exception(&if_stmt.body) {
        result.extend(
            RaisingGuard {
                test: if_stmt.test.as_ref(),
                raised_exception,
            }
            .rule(env),
        );
    }

    for clause in &if_stmt.elif_else_clauses {
        let Some(test) = &clause.test else {
            continue;
        };
        let Some(raised_exception) = direct_raise_exception(&clause.body) else {
            continue;
        };

        result.extend(
            RaisingGuard {
                test,
                raised_exception,
            }
            .rule(env),
        );
    }

    // NOTE: We do NOT recurse into nested if-statements here — that's handled
    // by the caller (process_statements) as it walks into the body/clauses.
    result
}

struct RaisingGuard<'a> {
    test: &'a Expr,
    raised_exception: &'a Expr,
}

impl RaisingGuard<'_> {
    fn rule(self, env: &mut Env) -> ExtractedRuleFragment {
        let constraints = eval_condition(self.test, env);
        let mut diagnostic_messages = Vec::new();

        if let Some(message) = extract_exception_message(self.raised_exception, env) {
            diagnostic_messages.extend(constraints.arg_constraints.iter().cloned().map(
                |constraint| ExtractedDiagnosticMessage {
                    constraint: ExtractedDiagnosticConstraint::ArgumentCount(constraint),
                    message: message.clone(),
                },
            ));

            diagnostic_messages.extend(constraints.required_keywords.iter().map(|keyword| {
                ExtractedDiagnosticMessage {
                    constraint: ExtractedDiagnosticConstraint::RequiredKeyword {
                        position: keyword.position,
                        value: keyword.value.clone(),
                    },
                    message: message.clone(),
                }
            }));

            diagnostic_messages.extend(constraints.choice_at_constraints.iter().map(|choice| {
                ExtractedDiagnosticMessage {
                    constraint: ExtractedDiagnosticConstraint::ChoiceAt {
                        position: choice.position,
                        values: choice.values.clone(),
                    },
                    message: message.clone(),
                }
            }));
        }

        ExtractedRuleFragment {
            constraints,
            diagnostic_messages,
        }
    }
}

/// Evaluate a condition expression as a constraint.
///
/// The condition guards a `raise` statement, so it describes when the code
/// errors. Constraints capture what's valid (the negation).
fn eval_condition(expr: &Expr, env: &mut Env) -> ExtractedTagConstraints {
    match expr {
        // `or`: error when either side is true → each is an independent constraint
        Expr::BoolOp(ExprBoolOp {
            op: BoolOp::Or,
            values,
            ..
        }) => values
            .iter()
            .fold(ExtractedTagConstraints::default(), |acc, value| {
                acc.or(eval_condition(value, env))
            }),

        // `and`: error when both true → length constraints are protective guards,
        // discard them but keep keyword constraints
        Expr::BoolOp(ExprBoolOp {
            op: BoolOp::And,
            values,
            ..
        }) => values
            .iter()
            .fold(ExtractedTagConstraints::default(), |acc, value| {
                acc.and(eval_condition(value, env))
            }),

        // Comparison: `len(bits) < 4` or `bits[2] != "as"`
        Expr::Compare(compare) => eval_compare(compare, env),

        // Negation: `not (2 <= len(bits) <= 4)` or `not len(bits) == 3`
        Expr::UnaryOp(ExprUnaryOp {
            op: UnaryOp::Not,
            operand,
            ..
        }) => {
            if let Expr::Compare(compare) = operand.as_ref() {
                eval_negated_compare(compare, env)
            } else {
                ExtractedTagConstraints::default()
            }
        }

        _ => ExtractedTagConstraints::default(),
    }
}

fn eval_compare(compare: &ExprCompare, env: &mut Env) -> ExtractedTagConstraints {
    if compare.ops.is_empty() || compare.comparators.is_empty() {
        return ExtractedTagConstraints::default();
    }

    // Skip chained comparisons (e.g., `2 <= len(bits) <= 4`). The only
    // meaningful chained comparison in error guards is the negated form
    // (`not (2 <= len(bits) <= 4)`), handled by eval_negated_compare.
    // Processing only the first pair of a chain produces wrong constraints.
    if compare.ops.len() > 1 {
        return ExtractedTagConstraints::default();
    }

    let op = &compare.ops[0];
    let left = &compare.left;
    let comparator = &compare.comparators[0];

    let left_val = eval_expr(left, env);
    let right_val = eval_expr(comparator, env);

    // len(split_result) vs integer
    if let AbstractValue::SplitLength(split) = &left_val {
        if let Some(n) = comparator.non_negative_integer() {
            let constraint = match op {
                CmpOp::NotEq => Some(ArgumentCountConstraint::Exact(split.resolve_length(n))),
                CmpOp::Lt => Some(ArgumentCountConstraint::Min(split.resolve_length(n))),
                CmpOp::LtE => Some(ArgumentCountConstraint::Min(split.resolve_length(n + 1))),
                CmpOp::Gt => Some(ArgumentCountConstraint::Max(split.resolve_length(n))),
                CmpOp::GtE if n > 0 => {
                    Some(ArgumentCountConstraint::Max(split.resolve_length(n - 1)))
                }
                _ => None,
            };
            return constraint.map_or_else(
                ExtractedTagConstraints::default,
                ExtractedTagConstraints::single_length,
            );
        }

        // `len(bits) not in (2, 3, 4)` → valid counts are {2+offset, 3+offset, 4+offset}
        if matches!(op, CmpOp::NotIn)
            && let Some(values) = comparator.collection_map(ExprExt::non_negative_integer)
        {
            return ExtractedTagConstraints::single_length(ArgumentCountConstraint::OneOf(
                values
                    .into_iter()
                    .map(|v| split.resolve_length(v))
                    .collect(),
            ));
        }
        return ExtractedTagConstraints::default();
    }

    // Reversed: integer vs len(split_result), e.g. `4 < len(bits)`
    if let AbstractValue::SplitLength(split) = &right_val {
        if let Some(n) = left.non_negative_integer() {
            let constraint = match op {
                CmpOp::Lt => Some(ArgumentCountConstraint::Max(split.resolve_length(n))),
                CmpOp::LtE if n > 0 => {
                    Some(ArgumentCountConstraint::Max(split.resolve_length(n - 1)))
                }
                CmpOp::Gt => Some(ArgumentCountConstraint::Min(split.resolve_length(n))),
                CmpOp::GtE => Some(ArgumentCountConstraint::Min(split.resolve_length(n + 1))),
                _ => None,
            };
            return constraint.map_or_else(
                ExtractedTagConstraints::default,
                ExtractedTagConstraints::single_length,
            );
        }
        return ExtractedTagConstraints::default();
    }

    // SplitElement vs string: `bits[N] != "keyword"`
    if let AbstractValue::SplitElement { index } = left_val {
        if let Some(keyword) = comparator.string_literal() {
            if matches!(op, CmpOp::NotEq) {
                let position = index;
                return ExtractedTagConstraints::single_keyword(RequiredKeyword {
                    position,
                    value: keyword,
                });
            }
            return ExtractedTagConstraints::default();
        }

        // SplitElement not in ("a", "b") → ChoiceAt constraint
        if matches!(op, CmpOp::NotIn)
            && let Some(values) = comparator.collection_map(ExprExt::string_literal)
            && !values.is_empty()
        {
            let position = index;
            return ExtractedTagConstraints::single_choice(ChoiceAt { position, values });
        }
        return ExtractedTagConstraints::default();
    }

    // Reversed: string vs SplitElement: `"keyword" != bits[N]`
    if let AbstractValue::SplitElement { index } = right_val
        && let Some(keyword) = left.string_literal()
    {
        if matches!(op, CmpOp::NotEq) {
            let position = index;
            return ExtractedTagConstraints::single_keyword(RequiredKeyword {
                position,
                value: keyword,
            });
        }
        return ExtractedTagConstraints::default();
    }

    ExtractedTagConstraints::default()
}

fn eval_negated_compare(compare: &ExprCompare, env: &mut Env) -> ExtractedTagConstraints {
    // Range: `not (2 <= len(bits) <= 4)` → valid range is min..=max
    if compare.ops.len() == 2
        && compare.comparators.len() == 2
        && let Some(range_constraints) = eval_range_constraint(compare, env)
    {
        return ExtractedTagConstraints {
            arg_constraints: range_constraints,
            ..Default::default()
        };
    }

    // Simple negation: `not len(bits) == 3` → Exact(3)
    if compare.ops.len() == 1 && compare.comparators.len() == 1 {
        let left_val = eval_expr(&compare.left, env);
        if let AbstractValue::SplitLength(split) = left_val
            && let Some(n) = compare.comparators[0].non_negative_integer()
        {
            let constraint = match &compare.ops[0] {
                CmpOp::Eq => Some(ArgumentCountConstraint::Exact(split.resolve_length(n))),
                CmpOp::Lt if n > 0 => {
                    Some(ArgumentCountConstraint::Max(split.resolve_length(n - 1)))
                }
                CmpOp::Gt => Some(ArgumentCountConstraint::Min(split.resolve_length(n + 1))),
                _ => None,
            };
            if let Some(c) = constraint {
                return ExtractedTagConstraints::single_length(c);
            }
        }
    }

    ExtractedTagConstraints::default()
}

/// Extract range constraint from negated `not (CONST <=/<  len(var) <=/<  CONST)`.
///
/// Only valid in negated context: `not (2 <= len(bits) <= 4)` means "error when
/// NOT in [2,4]", so the valid range IS [2,4] → `Min(2), Max(4)`.
fn eval_range_constraint(
    compare: &ExprCompare,
    env: &mut Env,
) -> Option<Vec<ArgumentCountConstraint>> {
    if compare.ops.len() != 2 || compare.comparators.len() != 2 {
        return None;
    }

    let middle = eval_expr(&compare.comparators[0], env);
    let AbstractValue::SplitLength(split) = middle else {
        return None;
    };
    let lower = compare.left.non_negative_integer()?;
    let upper = compare.comparators[1].non_negative_integer()?;

    let op1 = &compare.ops[0];
    let op2 = &compare.ops[1];

    if !matches!(op1, CmpOp::Lt | CmpOp::LtE) || !matches!(op2, CmpOp::Lt | CmpOp::LtE) {
        return None;
    }

    let min_val = if matches!(op1, CmpOp::LtE) {
        lower
    } else {
        lower + 1
    };
    let max_val = if matches!(op2, CmpOp::LtE) {
        upper
    } else {
        upper.checked_sub(1)?
    };

    if min_val > max_val {
        return None;
    }

    Some(vec![
        ArgumentCountConstraint::Min(split.resolve_length(min_val)),
        ArgumentCountConstraint::Max(split.resolve_length(max_val)),
    ])
}

#[cfg(test)]
mod tests {
    use ruff_python_ast::Stmt;
    use ruff_python_ast::StmtFunctionDef;
    use ruff_python_parser::parse_module;

    use super::*;
    use crate::specs::analysis::AnalysisResult;
    use crate::specs::analysis::CallContext;
    use crate::specs::analysis::state::Env;
    use crate::specs::analysis::statements::process_statements;
    use crate::specs::testing::django_function;
    use crate::specs::types::ExtractedMessageArg;
    use crate::specs::types::ExtractedMessageTemplate;
    use crate::specs::types::SplitPosition;

    fn extract_from_source(source: &str) -> ExtractedTagConstraints {
        extract_result_from_source(source).constraints
    }

    fn extract_result_from_source(source: &str) -> AnalysisResult {
        let parsed = parse_module(source).expect("valid Python");
        let module = parsed.into_syntax();
        let func = module
            .body
            .into_iter()
            .find_map(|s| {
                if let Stmt::FunctionDef(f) = s {
                    Some(f)
                } else {
                    None
                }
            })
            .expect("no function found");

        extract_result_from_func(&func)
    }

    fn extract_from_func(func: &StmtFunctionDef) -> ExtractedTagConstraints {
        extract_result_from_func(func).constraints
    }

    fn extract_result_from_func(func: &StmtFunctionDef) -> AnalysisResult {
        let parser_param = func
            .parameters
            .args
            .first()
            .map_or("parser", |p| p.parameter.name.as_str());
        let token_param = func
            .parameters
            .args
            .get(1)
            .map_or("token", |p| p.parameter.name.as_str());

        let mut env = Env::for_compile_function(parser_param, token_param);
        let mut ctx = CallContext {
            db: None,
            file: None,
        };
        process_statements(&func.body, &mut env, &mut ctx)
    }

    // Fabricated: tests isolated `<` comparator on len(bits). Real Django functions
    // combine this with other guards (e.g., do_for has len < 4 plus keyword checks),
    // making isolated operator testing impractical with corpus source.
    #[test]
    fn len_lt() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) < 2:
        raise TemplateSyntaxError("err")
"#,
        );
        assert_eq!(c.arg_constraints, vec![ArgumentCountConstraint::Min(2)]);
    }

    #[test]
    fn extracts_static_exception_message_for_constraint() {
        let result = extract_result_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) < 2:
        raise TemplateSyntaxError("custom tag needs one argument")
"#,
        );

        assert_eq!(
            result.constraints.arg_constraints,
            vec![ArgumentCountConstraint::Min(2)]
        );
        assert_eq!(
            result.diagnostic_messages,
            vec![ExtractedDiagnosticMessage {
                constraint: ExtractedDiagnosticConstraint::ArgumentCount(
                    ArgumentCountConstraint::Min(2)
                ),
                message: ExtractedMessageTemplate::Static(
                    "custom tag needs one argument".to_string()
                ),
            }]
        );
    }

    #[test]
    fn extracts_percent_formatted_exception_message_for_constraint() {
        let result = extract_result_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) < 2:
        raise TemplateSyntaxError("'%s' takes at least one argument" % bits[0])
"#,
        );

        assert_eq!(
            result.constraints.arg_constraints,
            vec![ArgumentCountConstraint::Min(2)]
        );
        assert_eq!(
            result.diagnostic_messages,
            vec![ExtractedDiagnosticMessage {
                constraint: ExtractedDiagnosticConstraint::ArgumentCount(
                    ArgumentCountConstraint::Min(2)
                ),
                message: ExtractedMessageTemplate::PercentFormat {
                    template: "'%s' takes at least one argument".to_string(),
                    args: vec![ExtractedMessageArg::SplitElement(SplitPosition::Forward(0))],
                },
            }]
        );
    }

    // Fabricated: tests isolated `!=` comparator. Real functions with len != N
    // (e.g., regroup, templatetag) also have keyword checks; tested end-to-end
    // in regroup_pattern_end_to_end and corpus_regroup below.
    #[test]
    fn len_ne() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) != 4:
        raise TemplateSyntaxError("err")
"#,
        );
        assert_eq!(c.arg_constraints, vec![ArgumentCountConstraint::Exact(4)]);
    }

    // Fabricated: tests isolated `>` comparator. resetcycle has `len(args) > 2`
    // but doesn't raise TemplateSyntaxError for it directly in a guard form
    // our analyzer recognizes.
    #[test]
    fn len_gt() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) > 5:
        raise TemplateSyntaxError("err")
"#,
        );
        assert_eq!(c.arg_constraints, vec![ArgumentCountConstraint::Max(5)]);
    }

    // Fabricated: tests `<=` comparator — rare in Django, no clean corpus example.
    #[test]
    fn len_le() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) <= 1:
        raise TemplateSyntaxError("err")
"#,
        );
        assert_eq!(c.arg_constraints, vec![ArgumentCountConstraint::Min(2)]);
    }

    // Fabricated: tests `>=` comparator — rare in Django, no clean corpus example.
    #[test]
    fn len_ge() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) >= 5:
        raise TemplateSyntaxError("err")
"#,
        );
        assert_eq!(c.arg_constraints, vec![ArgumentCountConstraint::Max(4)]);
    }

    // Fabricated: tests reversed comparison `N < len(bits)` — tests comparator
    // normalization logic. No corpus function uses this form.
    #[test]
    fn reversed_lt() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if 4 < len(bits):
        raise TemplateSyntaxError("err")
"#,
        );
        assert_eq!(c.arg_constraints, vec![ArgumentCountConstraint::Max(4)]);
    }

    // Fabricated: tests reversed comparison `N > len(bits)` — tests comparator
    // normalization logic. No corpus function uses this form.
    #[test]
    fn reversed_gt() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if 4 > len(bits):
        raise TemplateSyntaxError("err")
"#,
        );
        assert_eq!(c.arg_constraints, vec![ArgumentCountConstraint::Min(4)]);
    }

    // Fabricated: tests isolated keyword extraction from `bits[N] != "keyword"`.
    // Real functions combine this with len checks; tested end-to-end in
    // corpus_regroup and corpus_get_current_timezone.
    #[test]
    fn required_keyword_ne() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if bits[2] != "as":
        raise TemplateSyntaxError("err")
"#,
        );
        assert!(c.arg_constraints.is_empty());
        assert_eq!(
            c.required_keywords,
            vec![RequiredKeyword {
                position: SplitPosition::Forward(2),
                value: "as".to_string()
            }]
        );
    }

    // Fabricated: tests negative index keyword extraction — `bits[-1] != "silent"`.
    // No corpus function has this exact isolated pattern.
    #[test]
    fn required_keyword_backward() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if bits[-1] != "silent":
        raise TemplateSyntaxError("err")
"#,
        );
        assert_eq!(
            c.required_keywords,
            vec![RequiredKeyword {
                position: SplitPosition::Backward(1),
                value: "silent".to_string()
            }]
        );
    }

    // Fabricated: tests `or` boolean operator producing independent constraints.
    // Real examples (e.g., l10n.py localize_tag) use `or` with `not in` which
    // produces ChoiceAt, not RequiredKeyword. Tested end-to-end in
    // corpus_get_current_timezone.
    #[test]
    fn compound_or() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) != 3 or bits[1] != "as":
        raise TemplateSyntaxError("err")
"#,
        );
        assert_eq!(c.arg_constraints, vec![ArgumentCountConstraint::Exact(3)]);
        assert_eq!(
            c.required_keywords,
            vec![RequiredKeyword {
                position: SplitPosition::Forward(1),
                value: "as".to_string()
            }]
        );
    }

    // Fabricated: tests `and` semantics — length discarded, keyword kept.
    // Tests boolean operator handling logic.
    #[test]
    fn compound_and_discards_length() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) > 3 and bits[2] != "as":
        raise TemplateSyntaxError("err")
"#,
        );
        // Length discarded under `and`, only keyword kept
        assert!(c.arg_constraints.is_empty());
        assert_eq!(
            c.required_keywords,
            vec![RequiredKeyword {
                position: SplitPosition::Forward(2),
                value: "as".to_string()
            }]
        );
    }

    // Fabricated: tests `not (N <= len(bits) <= M)` range negation.
    // No corpus function uses this exact negated-range form (flatpages uses
    // the positive form `3 <= len(bits) <= 6` without negation).
    #[test]
    fn negated_range() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if not (2 <= len(bits) <= 4):
        raise TemplateSyntaxError("err")
"#,
        );
        assert_eq!(
            c.arg_constraints,
            vec![
                ArgumentCountConstraint::Min(2),
                ArgumentCountConstraint::Max(4)
            ]
        );
    }

    // Fabricated: tests `len(bits) not in (2, 3, 4)` pattern.
    // No corpus function uses this pattern — it's a valid Django API but
    // no real tag uses `not in` with a tuple of allowed counts.
    #[test]
    fn len_not_in() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) not in (2, 3, 4):
        raise TemplateSyntaxError("err")
"#,
        );
        assert_eq!(
            c.arg_constraints,
            vec![ArgumentCountConstraint::OneOf(vec![2, 3, 4])]
        );
    }

    // Fabricated: tests slice offset arithmetic — `bits = bits[1:]` followed by
    // `len(bits) < 3` should produce Min(4) due to offset adjustment. Tests
    // internal tracking logic.
    #[test]
    fn offset_adjustment_after_slice() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    bits = bits[1:]
    if len(bits) < 3:
        raise TemplateSyntaxError("err")
"#,
        );
        // len(bits) < 3 with base_offset=1 → Min(3+1) = Min(4)
        assert_eq!(c.arg_constraints, vec![ArgumentCountConstraint::Min(4)]);
    }

    // Fabricated: tests multiple sequential if/raise producing multiple
    // constraints. Real functions have this pattern but always mixed with
    // other code; tested end-to-end in corpus_regroup.
    #[test]
    fn multiple_raises() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) < 2:
        raise TemplateSyntaxError("too few")
    if len(bits) > 5:
        raise TemplateSyntaxError("too many")
"#,
        );
        assert_eq!(
            c.arg_constraints,
            vec![
                ArgumentCountConstraint::Min(2),
                ArgumentCountConstraint::Max(5)
            ]
        );
    }

    // Fabricated: tests nested if producing keyword constraint from inner guard.
    // Real functions nest ifs but always with additional logic.
    #[test]
    fn nested_if_raise() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) >= 3:
        if bits[2] != "as":
            raise TemplateSyntaxError("err")
"#,
        );
        assert_eq!(
            c.required_keywords,
            vec![RequiredKeyword {
                position: SplitPosition::Forward(2),
                value: "as".to_string()
            }]
        );
    }

    // Fabricated: tests elif producing constraints from both branches.
    // Real elif guards exist but are mixed with other code.
    #[test]
    fn elif_raise() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) < 2:
        raise TemplateSyntaxError("too few")
    elif len(bits) > 4:
        raise TemplateSyntaxError("too many")
"#,
        );
        assert_eq!(
            c.arg_constraints,
            vec![
                ArgumentCountConstraint::Min(2),
                ArgumentCountConstraint::Max(4)
            ]
        );
    }

    // Fabricated: tests that non-TemplateSyntaxError raises are also extracted.
    // Any uncaught raise in an if-body is a validation constraint.
    #[test]
    fn non_template_syntax_error_extracted() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) < 2:
        raise ValueError("not a template error")
"#,
        );
        assert_eq!(c.arg_constraints, vec![ArgumentCountConstraint::Min(2)]);
    }

    // Fabricated: tests qualified exception raise `module.SomeError(...)`.
    // Some libraries use `from django.template import exceptions` style.
    #[test]
    fn qualified_exception_extracted() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) != 3:
        raise template.TemplateSyntaxError("err")
"#,
        );
        assert_eq!(c.arg_constraints, vec![ArgumentCountConstraint::Exact(3)]);
    }

    // Fabricated: tests TypeError raise — seen in corpus (e.g., wagtail).
    #[test]
    fn type_error_extracted() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if bits[1] != "as":
        raise TypeError("expected 'as' keyword")
"#,
        );
        assert_eq!(
            c.required_keywords,
            vec![RequiredKeyword {
                position: SplitPosition::Forward(1),
                value: "as".to_string()
            }]
        );
    }

    // Fabricated: tests RuntimeError raise — any exception type should work.
    #[test]
    fn runtime_error_extracted() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) > 4:
        raise RuntimeError("too many arguments")
"#,
        );
        assert_eq!(c.arg_constraints, vec![ArgumentCountConstraint::Max(4)]);
    }

    // Fabricated: tests bare `raise` (re-raise) is NOT extracted — only
    // raises with an explicit exception expression count.
    #[test]
    fn bare_raise_not_extracted() {
        let c = extract_from_source(
            r"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) < 2:
        raise
",
        );
        assert!(c.arg_constraints.is_empty());
    }

    // Fabricated: tests mixed exception types across multiple if/raise guards.
    #[test]
    fn mixed_exception_types() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) < 2:
        raise TemplateSyntaxError("too few")
    if len(bits) > 5:
        raise ValueError("too many")
"#,
        );
        assert_eq!(
            c.arg_constraints,
            vec![
                ArgumentCountConstraint::Min(2),
                ArgumentCountConstraint::Max(5)
            ]
        );
    }

    // Corpus: regroup in defaulttags.py — `len(bits) != 6`, `bits[2] != "by"`,
    // `bits[4] != "as"` in sequential if/raise guards.
    #[test]
    fn regroup_pattern_end_to_end() {
        let func = django_function("django/template/defaulttags.py", "regroup").unwrap();
        let c = extract_from_func(&func);
        assert_eq!(c.arg_constraints, vec![ArgumentCountConstraint::Exact(6)]);
        assert_eq!(
            c.required_keywords,
            vec![
                RequiredKeyword {
                    position: SplitPosition::Forward(2),
                    value: "by".to_string()
                },
                RequiredKeyword {
                    position: SplitPosition::Forward(4),
                    value: "as".to_string()
                }
            ]
        );
    }

    // Fabricated: tests that unknown variable produces no constraint.
    // Robustness test for abstract interpreter.
    #[test]
    fn unknown_variable_produces_no_constraint() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    if len(unknown_var) < 2:
        raise TemplateSyntaxError("err")
"#,
        );
        assert!(c.arg_constraints.is_empty());
    }

    // Fabricated: tests reversed string comparison `"as" != bits[2]`.
    // No corpus function uses reversed form — tests comparator normalization.
    #[test]
    fn keyword_from_reversed_comparison() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if "as" != bits[2]:
        raise TemplateSyntaxError("err")
"#,
        );
        assert_eq!(
            c.required_keywords,
            vec![RequiredKeyword {
                position: SplitPosition::Forward(2),
                value: "as".to_string()
            }]
        );
    }

    // Fabricated: tests star unpack offset — `tag_name, *rest = bits` makes
    // `rest` have base_offset=1. No corpus function has star unpack followed
    // by an isolated len guard + TemplateSyntaxError raise.
    #[test]
    fn star_unpack_then_constraint() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    tag_name, *rest = token.split_contents()
    if len(rest) < 2:
        raise TemplateSyntaxError("err")
"#,
        );
        // rest has base_offset=1, so Min(2+1) = Min(3)
        assert_eq!(c.arg_constraints, vec![ArgumentCountConstraint::Min(3)]);
    }

    // Fabricated: tests pop(0) offset tracking — after pop(0), base_offset=1.
    // No corpus function has pop(0) followed by an isolated len guard;
    // real uses (e.g., i18n.py) pop in while-loops which are handled differently.
    #[test]
    fn pop_0_offset_adjusted_constraint() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    bits.pop(0)
    if len(bits) < 3:
        raise TemplateSyntaxError("err")
"#,
        );
        // len(bits) < 3 where bits has base_offset=1 → Min(3+1) = Min(4)
        assert_eq!(c.arg_constraints, vec![ArgumentCountConstraint::Min(4)]);
    }

    // Fabricated: tests multiple pop() from end — pops_from_end tracking.
    // Tests offset arithmetic correctness.
    #[test]
    fn end_pop_adjusted_constraint() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    bits.pop()
    bits.pop()
    if len(bits) != 1:
        raise TemplateSyntaxError("err")
"#,
        );
        // len(bits) != 1 where bits has pops_from_end=2 → Exact(1+0+2) = Exact(3)
        assert_eq!(c.arg_constraints, vec![ArgumentCountConstraint::Exact(3)]);
    }

    // Fabricated: tests combined pop(0) + pop() offset arithmetic.
    // Both base_offset and pops_from_end contribute to final constraint.
    #[test]
    fn combined_pop_front_and_end() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    bits.pop(0)
    bits.pop()
    if len(bits) < 2:
        raise TemplateSyntaxError("err")
"#,
        );
        // base_offset=1, pops_from_end=1 → Min(2+1+1) = Min(4)
        assert_eq!(c.arg_constraints, vec![ArgumentCountConstraint::Min(4)]);
    }

    // Fabricated: tests pop(0) with assignment — `tag_name = bits.pop(0)`.
    // Tests that assignment doesn't prevent offset tracking.
    #[test]
    fn pop_0_with_assignment_then_constraint() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    tag_name = bits.pop(0)
    if len(bits) < 2:
        raise TemplateSyntaxError("err")
"#,
        );
        // After pop(0): base_offset=1 → Min(2+1) = Min(3)
        assert_eq!(c.arg_constraints, vec![ArgumentCountConstraint::Min(3)]);
    }

    // Fabricated: tests isolated ChoiceAt extraction from `not in` tuple.
    // Real functions (autoescape, localize_tag) combine this with len checks;
    // tested end-to-end in corpus_autoescape below.
    #[test]
    fn choice_at_not_in_tuple() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    args = token.split_contents()
    if args[1] not in ("on", "off"):
        raise TemplateSyntaxError("err")
"#,
        );
        assert!(c.arg_constraints.is_empty());
        assert!(c.required_keywords.is_empty());
        assert_eq!(
            c.choice_at_constraints,
            vec![ChoiceAt {
                position: SplitPosition::Forward(1),
                values: vec!["on".to_string(), "off".to_string()]
            }]
        );
    }

    // Corpus: autoescape in defaulttags.py — `len(args) != 2` followed by
    // `arg not in ("on", "off")` where `arg = args[1]`.
    // Uses `token.contents.split()` (not split_contents), which the abstract
    // interpreter handles equivalently.
    #[test]
    fn choice_at_autoescape_pattern() {
        let func = django_function("django/template/defaulttags.py", "autoescape").unwrap();
        let c = extract_from_func(&func);
        assert_eq!(c.arg_constraints, vec![ArgumentCountConstraint::Exact(2)]);
        assert_eq!(
            c.choice_at_constraints,
            vec![ChoiceAt {
                position: SplitPosition::Forward(1),
                values: vec!["on".to_string(), "off".to_string()]
            }]
        );
    }

    // Fabricated: tests ChoiceAt with list literal `["a", "b", "c"]` instead of
    // tuple. No corpus function uses list syntax for `not in` checks — all use
    // tuples. Tests that both collection types are handled.
    #[test]
    fn choice_at_with_list() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if bits[1] not in ["open", "close", "block"]:
        raise TemplateSyntaxError("err")
"#,
        );
        assert_eq!(
            c.choice_at_constraints,
            vec![ChoiceAt {
                position: SplitPosition::Forward(1),
                values: vec!["open".to_string(), "close".to_string(), "block".to_string()]
            }]
        );
    }

    // Fabricated: tests ChoiceAt with negative index `bits[-1]`.
    // No corpus function uses negative index with `not in`.
    #[test]
    fn choice_at_negative_index() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if bits[-1] not in ("yes", "no"):
        raise TemplateSyntaxError("err")
"#,
        );
        assert_eq!(
            c.choice_at_constraints,
            vec![ChoiceAt {
                position: SplitPosition::Backward(1),
                values: vec!["yes".to_string(), "no".to_string()]
            }]
        );
    }

    // Fabricated: tests boundary — single string `!=` produces RequiredKeyword
    // not ChoiceAt. Tests classification logic.
    #[test]
    fn no_choice_at_for_single_string() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if bits[1] != "as":
        raise TemplateSyntaxError("err")
"#,
        );
        assert!(c.choice_at_constraints.is_empty());
        assert_eq!(c.required_keywords.len(), 1);
    }

    // Corpus: get_current_timezone in tz.py — compound `or` guard:
    // `len(args) != 3 or args[1] != "as"` using `token.contents.split()`.
    // Produces Exact(3) + RequiredKeyword("as" at position 1).
    #[test]
    fn corpus_get_current_timezone() {
        let func =
            django_function("django/templatetags/tz.py", "get_current_timezone_tag").unwrap();
        let c = extract_from_func(&func);
        assert_eq!(c.arg_constraints, vec![ArgumentCountConstraint::Exact(3)]);
        assert_eq!(
            c.required_keywords,
            vec![RequiredKeyword {
                position: SplitPosition::Forward(1),
                value: "as".to_string()
            }]
        );
    }

    // Corpus: timezone_tag in tz.py — simple `len(bits) != 2` guard.
    // Clean single-constraint example.
    #[test]
    fn corpus_timezone_tag() {
        let func = django_function("django/templatetags/tz.py", "timezone_tag").unwrap();
        let c = extract_from_func(&func);
        assert_eq!(c.arg_constraints, vec![ArgumentCountConstraint::Exact(2)]);
        assert!(c.required_keywords.is_empty());
        assert!(c.choice_at_constraints.is_empty());
    }

    // Corpus: do_for in defaulttags.py — `len(bits) < 4` produces Min(4).
    // Also has keyword checks but they use computed index (in_index) which
    // the abstract interpreter may not fully resolve.
    #[test]
    fn corpus_do_for() {
        let func = django_function("django/template/defaulttags.py", "do_for").unwrap();
        let c = extract_from_func(&func);
        assert!(c.arg_constraints.contains(&ArgumentCountConstraint::Min(4)));
    }

    // Corpus: cycle in defaulttags.py — `len(args) < 2` produces Min(2).
    #[test]
    fn corpus_cycle() {
        let func = django_function("django/template/defaulttags.py", "cycle").unwrap();
        let c = extract_from_func(&func);
        assert!(c.arg_constraints.contains(&ArgumentCountConstraint::Min(2)));
    }

    // Corpus: url in defaulttags.py — `len(bits) < 2` produces Min(2).
    #[test]
    fn corpus_url() {
        let func = django_function("django/template/defaulttags.py", "url").unwrap();
        let c = extract_from_func(&func);
        assert!(c.arg_constraints.contains(&ArgumentCountConstraint::Min(2)));
    }

    // Corpus: localtime_tag in tz.py — compound `or` with ChoiceAt:
    // `len(bits) > 2 or bits[1] not in ("on", "off")` produces
    // Max(2) + ChoiceAt(position=1, ["on", "off"]).
    #[test]
    fn corpus_localtime_tag() {
        let func = django_function("django/templatetags/tz.py", "localtime_tag").unwrap();
        let c = extract_from_func(&func);
        assert!(c.arg_constraints.contains(&ArgumentCountConstraint::Max(2)));
        assert_eq!(
            c.choice_at_constraints,
            vec![ChoiceAt {
                position: SplitPosition::Forward(1),
                values: vec!["on".to_string(), "off".to_string()]
            }]
        );
    }
}

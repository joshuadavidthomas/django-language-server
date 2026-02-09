//! Constraint extraction from if/raise conditions.
//!
//! Finds `if condition: raise TemplateSyntaxError(...)` patterns and
//! interprets the condition against the abstract environment to produce
//! `ArgumentCountConstraint` and `RequiredKeyword` values.

use ruff_python_ast::BoolOp;
use ruff_python_ast::CmpOp;
use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprBoolOp;
use ruff_python_ast::ExprCall;
use ruff_python_ast::ExprCompare;
use ruff_python_ast::ExprName;
use ruff_python_ast::ExprUnaryOp;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtIf;
use ruff_python_ast::StmtRaise;
use ruff_python_ast::UnaryOp;

use super::domain::AbstractValue;
use super::domain::Env;
use super::eval::eval_expr;
use crate::ext::ExprExt;
use crate::types::ArgumentCountConstraint;
use crate::types::ChoiceAt;
use crate::types::RequiredKeyword;

/// Collected constraints from analyzing a function body.
///
/// Provides algebraic `or()` and `and()` methods that encode boolean
/// composition semantics from if/raise guard analysis:
/// - `or`: error when either side is true → each constraint is independent
/// - `and`: both must be true for error → length constraints dropped, keywords/choices kept
#[derive(Debug, Clone, Default)]
#[allow(clippy::struct_field_names)]
pub struct ConstraintSet {
    pub arg_constraints: Vec<ArgumentCountConstraint>,
    pub required_keywords: Vec<RequiredKeyword>,
    pub choice_at_constraints: Vec<ChoiceAt>,
}

impl ConstraintSet {
    pub fn single_length(c: ArgumentCountConstraint) -> Self {
        Self {
            arg_constraints: vec![c],
            ..Default::default()
        }
    }

    pub fn single_keyword(k: RequiredKeyword) -> Self {
        Self {
            required_keywords: vec![k],
            ..Default::default()
        }
    }

    pub fn single_choice(c: ChoiceAt) -> Self {
        Self {
            choice_at_constraints: vec![c],
            ..Default::default()
        }
    }

    /// Disjunction: error when either side is true → each is independent.
    pub fn or(mut self, other: Self) -> Self {
        self.arg_constraints.extend(other.arg_constraints);
        self.required_keywords.extend(other.required_keywords);
        self.choice_at_constraints
            .extend(other.choice_at_constraints);
        self
    }

    /// Conjunction: both must be true for error → drop length constraints,
    /// keep keyword/choice constraints.
    pub fn and(self, other: Self) -> Self {
        let mut required_keywords = self.required_keywords;
        required_keywords.extend(other.required_keywords);
        let mut choice_at_constraints = self.choice_at_constraints;
        choice_at_constraints.extend(other.choice_at_constraints);
        Self {
            arg_constraints: Vec::new(),
            required_keywords,
            choice_at_constraints,
        }
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.arg_constraints.is_empty()
            && self.required_keywords.is_empty()
            && self.choice_at_constraints.is_empty()
    }

    pub fn extend(&mut self, other: Self) {
        self.arg_constraints.extend(other.arg_constraints);
        self.required_keywords.extend(other.required_keywords);
        self.choice_at_constraints
            .extend(other.choice_at_constraints);
    }
}

/// Extract constraints from a single if-statement using the current env state.
///
/// Called inline during statement processing so that constraints see the env
/// as it exists at the point in the code where the if-statement appears,
/// not the final env state after the entire function body has been processed.
pub fn extract_from_if_inline(if_stmt: &StmtIf, env: &Env) -> ConstraintSet {
    let mut result = ConstraintSet::default();

    if body_raises_template_syntax_error(&if_stmt.body) {
        result.extend(eval_condition(&if_stmt.test, env));
    }

    for clause in &if_stmt.elif_else_clauses {
        if body_raises_template_syntax_error(&clause.body) {
            if let Some(test) = &clause.test {
                result.extend(eval_condition(test, env));
            }
        }
    }

    // NOTE: We do NOT recurse into nested if-statements here — that's handled
    // by the caller (process_statements) as it walks into the body/clauses.
    result
}

/// Evaluate a condition expression as a constraint.
///
/// The condition guards a `raise TemplateSyntaxError(...)`, so it describes
/// when the code errors. Constraints capture what's valid (the negation).
fn eval_condition(expr: &Expr, env: &Env) -> ConstraintSet {
    match expr {
        // `or`: error when either side is true → each is an independent constraint
        Expr::BoolOp(ExprBoolOp {
            op: BoolOp::Or,
            values,
            ..
        }) => values.iter().fold(ConstraintSet::default(), |acc, value| {
            acc.or(eval_condition(value, env))
        }),

        // `and`: error when both true → length constraints are protective guards,
        // discard them but keep keyword constraints
        Expr::BoolOp(ExprBoolOp {
            op: BoolOp::And,
            values,
            ..
        }) => values.iter().fold(ConstraintSet::default(), |acc, value| {
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
                ConstraintSet::default()
            }
        }

        _ => ConstraintSet::default(),
    }
}

fn eval_compare(compare: &ExprCompare, env: &Env) -> ConstraintSet {
    if compare.ops.is_empty() || compare.comparators.is_empty() {
        return ConstraintSet::default();
    }

    // Skip chained comparisons (e.g., `2 <= len(bits) <= 4`). The only
    // meaningful chained comparison in error guards is the negated form
    // (`not (2 <= len(bits) <= 4)`), handled by eval_negated_compare.
    // Processing only the first pair of a chain produces wrong constraints.
    if compare.ops.len() > 1 {
        return ConstraintSet::default();
    }

    let op = &compare.ops[0];
    let left = &compare.left;
    let comparator = &compare.comparators[0];

    let left_val = eval_expr(left, env);
    let right_val = eval_expr(comparator, env);

    // len(split_result) vs integer
    if let AbstractValue::SplitLength(split) = &left_val {
        if let Some(n) = comparator.positive_integer() {
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
            return constraint.map_or_else(ConstraintSet::default, ConstraintSet::single_length);
        }

        // `len(bits) not in (2, 3, 4)` → valid counts are {2+offset, 3+offset, 4+offset}
        if matches!(op, CmpOp::NotIn) {
            if let Some(values) = comparator.collection_map(ExprExt::positive_integer) {
                return ConstraintSet::single_length(ArgumentCountConstraint::OneOf(
                    values
                        .into_iter()
                        .map(|v| split.resolve_length(v))
                        .collect(),
                ));
            }
        }
        return ConstraintSet::default();
    }

    // Reversed: integer vs len(split_result), e.g. `4 < len(bits)`
    if let AbstractValue::SplitLength(split) = &right_val {
        if let Some(n) = left.positive_integer() {
            let constraint = match op {
                CmpOp::Lt => Some(ArgumentCountConstraint::Max(split.resolve_length(n))),
                CmpOp::LtE if n > 0 => {
                    Some(ArgumentCountConstraint::Max(split.resolve_length(n - 1)))
                }
                CmpOp::Gt => Some(ArgumentCountConstraint::Min(split.resolve_length(n))),
                CmpOp::GtE => Some(ArgumentCountConstraint::Min(split.resolve_length(n + 1))),
                _ => None,
            };
            return constraint.map_or_else(ConstraintSet::default, ConstraintSet::single_length);
        }
        return ConstraintSet::default();
    }

    // SplitElement vs string: `bits[N] != "keyword"`
    if let AbstractValue::SplitElement { index } = &left_val {
        if let Some(keyword) = comparator.string_literal() {
            let position = *index;
            return ConstraintSet::single_keyword(RequiredKeyword {
                position,
                value: keyword,
            });
        }

        // SplitElement not in ("a", "b") → ChoiceAt constraint
        if matches!(op, CmpOp::NotIn) {
            if let Some(values) = comparator.collection_map(ExprExt::string_literal) {
                if !values.is_empty() {
                    let position = *index;
                    return ConstraintSet::single_choice(ChoiceAt { position, values });
                }
            }
        }
        return ConstraintSet::default();
    }

    // Reversed: string vs SplitElement: `"keyword" != bits[N]`
    if let AbstractValue::SplitElement { index } = &right_val {
        if let Some(keyword) = left.string_literal() {
            let position = *index;
            return ConstraintSet::single_keyword(RequiredKeyword {
                position,
                value: keyword,
            });
        }
    }

    ConstraintSet::default()
}

fn eval_negated_compare(compare: &ExprCompare, env: &Env) -> ConstraintSet {
    // Range: `not (2 <= len(bits) <= 4)` → valid range is min..=max
    if compare.ops.len() == 2 && compare.comparators.len() == 2 {
        if let Some(range_constraints) = eval_range_constraint(compare, env) {
            return ConstraintSet {
                arg_constraints: range_constraints,
                ..Default::default()
            };
        }
    }

    // Simple negation: `not len(bits) == 3` → Exact(3)
    if compare.ops.len() == 1 && compare.comparators.len() == 1 {
        let left_val = eval_expr(&compare.left, env);
        if let AbstractValue::SplitLength(split) = left_val {
            if let Some(n) = compare.comparators[0].positive_integer() {
                let constraint = match &compare.ops[0] {
                    CmpOp::Eq => {
                        Some(ArgumentCountConstraint::Exact(split.resolve_length(n)))
                    }
                    CmpOp::Lt if n > 0 => {
                        Some(ArgumentCountConstraint::Max(split.resolve_length(n - 1)))
                    }
                    CmpOp::Gt => {
                        Some(ArgumentCountConstraint::Min(split.resolve_length(n + 1)))
                    }
                    _ => None,
                };
                if let Some(c) = constraint {
                    return ConstraintSet::single_length(c);
                }
            }
        }
    }

    ConstraintSet::default()
}

/// Extract range constraint from negated `not (CONST <=/<  len(var) <=/<  CONST)`.
///
/// Only valid in negated context: `not (2 <= len(bits) <= 4)` means "error when
/// NOT in [2,4]", so the valid range IS [2,4] → `Min(2), Max(4)`.
fn eval_range_constraint(compare: &ExprCompare, env: &Env) -> Option<Vec<ArgumentCountConstraint>> {
    if compare.ops.len() != 2 || compare.comparators.len() != 2 {
        return None;
    }

    let middle = eval_expr(&compare.comparators[0], env);
    let AbstractValue::SplitLength(split) = middle else {
        return None;
    };
    let lower = compare.left.positive_integer()?;
    let upper = compare.comparators[1].positive_integer()?;

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

pub(super) fn body_raises_template_syntax_error(body: &[Stmt]) -> bool {
    for stmt in body {
        if let Stmt::Raise(StmtRaise { exc: Some(exc), .. }) = stmt {
            if is_template_syntax_error_call(exc) {
                return true;
            }
        }
    }
    false
}

pub(super) fn is_template_syntax_error_call(expr: &Expr) -> bool {
    let Expr::Call(ExprCall { func, .. }) = expr else {
        return false;
    };
    match func.as_ref() {
        Expr::Name(ExprName { id, .. }) => id.as_str() == "TemplateSyntaxError",
        Expr::Attribute(ExprAttribute { attr, .. }) => attr.as_str() == "TemplateSyntaxError",
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use ruff_python_ast::StmtFunctionDef;
    use ruff_python_parser::parse_module;

    use super::*;
    use crate::dataflow::calls::HelperCache;
    use crate::dataflow::domain::Env;
    use crate::dataflow::eval::process_statements;
    use crate::dataflow::eval::AnalysisContext;
    use crate::test_helpers::django_function;
    use crate::types::SplitPosition;

    fn extract_from_source(source: &str) -> ConstraintSet {
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

        extract_from_func(&func)
    }

    fn extract_from_func(func: &StmtFunctionDef) -> ConstraintSet {
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
        let mut cache = HelperCache::new();
        let mut ctx = AnalysisContext {
            module_funcs: &[],
            caller_name: func.name.as_str(),
            call_depth: 0,
            cache: &mut cache,
            known_options: None,
            constraints: ConstraintSet::default(),
        };
        process_statements(&func.body, &mut env, &mut ctx);
        ctx.constraints
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

    // Fabricated: tests that non-TemplateSyntaxError raises are ignored.
    // Robustness test — no corpus function raises ValueError in a guard.
    #[test]
    fn non_template_syntax_error_ignored() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) < 2:
        raise ValueError("not a template error")
"#,
        );
        assert!(c.arg_constraints.is_empty());
    }

    // Corpus: regroup in defaulttags.py — `len(bits) != 6`, `bits[2] != "by"`,
    // `bits[4] != "as"` in sequential if/raise guards.
    #[test]
    fn regroup_pattern_end_to_end() {
        let func = django_function("django/template/defaulttags.py", "regroup")
            .expect("corpus not synced");
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
        let func = django_function("django/template/defaulttags.py", "autoescape")
            .expect("corpus not synced");
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
        let func = django_function("django/templatetags/tz.py", "get_current_timezone_tag")
            .expect("corpus not synced");
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
        let func = django_function("django/templatetags/tz.py", "timezone_tag")
            .expect("corpus not synced");
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
        let func =
            django_function("django/template/defaulttags.py", "do_for").expect("corpus not synced");
        let c = extract_from_func(&func);
        assert!(c.arg_constraints.contains(&ArgumentCountConstraint::Min(4)));
    }

    // Corpus: cycle in defaulttags.py — `len(args) < 2` produces Min(2).
    #[test]
    fn corpus_cycle() {
        let func =
            django_function("django/template/defaulttags.py", "cycle").expect("corpus not synced");
        let c = extract_from_func(&func);
        assert!(c.arg_constraints.contains(&ArgumentCountConstraint::Min(2)));
    }

    // Corpus: url in defaulttags.py — `len(bits) < 2` produces Min(2).
    #[test]
    fn corpus_url() {
        let func =
            django_function("django/template/defaulttags.py", "url").expect("corpus not synced");
        let c = extract_from_func(&func);
        assert!(c.arg_constraints.contains(&ArgumentCountConstraint::Min(2)));
    }

    // Corpus: localtime_tag in tz.py — compound `or` with ChoiceAt:
    // `len(bits) > 2 or bits[1] not in ("on", "off")` produces
    // Max(2) + ChoiceAt(position=1, ["on", "off"]).
    #[test]
    fn corpus_localtime_tag() {
        let func = django_function("django/templatetags/tz.py", "localtime_tag")
            .expect("corpus not synced");
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

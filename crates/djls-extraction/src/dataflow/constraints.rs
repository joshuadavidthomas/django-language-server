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
use ruff_python_ast::ExprStringLiteral;
use ruff_python_ast::ExprUnaryOp;
use ruff_python_ast::Number;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtIf;
use ruff_python_ast::StmtRaise;
use ruff_python_ast::UnaryOp;

use super::domain::AbstractValue;
use super::domain::Env;
use super::domain::Index;
use super::eval::eval_expr;
use crate::types::ArgumentCountConstraint;
use crate::types::RequiredKeyword;

/// Collected constraints from analyzing a function body.
#[derive(Debug, Default)]
pub struct Constraints {
    pub arg_constraints: Vec<ArgumentCountConstraint>,
    pub required_keywords: Vec<RequiredKeyword>,
}

/// Extract constraints from a list of statements.
///
/// Finds `if condition: raise TemplateSyntaxError(...)` patterns and
/// interprets the condition against the abstract environment.
pub fn extract_constraints(stmts: &[Stmt], env: &Env) -> Constraints {
    let mut constraints = Constraints::default();
    extract_from_body(stmts, env, &mut constraints);
    constraints
}

fn extract_from_body(stmts: &[Stmt], env: &Env, constraints: &mut Constraints) {
    for stmt in stmts {
        if let Stmt::If(if_stmt) = stmt {
            extract_from_if(if_stmt, env, constraints);
        }
    }
}

fn extract_from_if(if_stmt: &StmtIf, env: &Env, constraints: &mut Constraints) {
    if body_raises_template_syntax_error(&if_stmt.body) {
        eval_condition(&if_stmt.test, env, constraints);
    }

    // Recurse into body for nested if-statements
    extract_from_body(&if_stmt.body, env, constraints);

    // Recurse into elif/else clauses
    for clause in &if_stmt.elif_else_clauses {
        if body_raises_template_syntax_error(&clause.body) {
            if let Some(test) = &clause.test {
                eval_condition(test, env, constraints);
            }
        }
        extract_from_body(&clause.body, env, constraints);
    }
}

/// Evaluate a condition expression as a constraint.
///
/// The condition guards a `raise TemplateSyntaxError(...)`, so it describes
/// when the code errors. Constraints capture what's valid (the negation).
fn eval_condition(expr: &Expr, env: &Env, constraints: &mut Constraints) {
    match expr {
        // `or`: error when either side is true → each is an independent constraint
        Expr::BoolOp(ExprBoolOp {
            op: BoolOp::Or,
            values,
            ..
        }) => {
            for value in values {
                eval_condition(value, env, constraints);
            }
        }

        // `and`: error when both true → length constraints are protective guards,
        // discard them but keep keyword constraints
        Expr::BoolOp(ExprBoolOp {
            op: BoolOp::And,
            values,
            ..
        }) => {
            let mut discarded = Vec::new();
            for value in values {
                let mut sub = Constraints::default();
                eval_condition(value, env, &mut sub);
                discarded.extend(sub.arg_constraints);
                constraints.required_keywords.extend(sub.required_keywords);
            }
        }

        // Comparison: `len(bits) < 4` or `bits[2] != "as"`
        Expr::Compare(compare) => {
            eval_compare(compare, env, constraints);
        }

        // Negation: `not (2 <= len(bits) <= 4)` or `not len(bits) == 3`
        Expr::UnaryOp(ExprUnaryOp {
            op: UnaryOp::Not,
            operand,
            ..
        }) => {
            if let Expr::Compare(compare) = operand.as_ref() {
                eval_negated_compare(compare, env, constraints);
            }
        }

        _ => {}
    }
}

fn eval_compare(compare: &ExprCompare, env: &Env, constraints: &mut Constraints) {
    if compare.ops.is_empty() || compare.comparators.is_empty() {
        return;
    }

    // Handle range comparisons: `2 <= len(bits) <= 4`
    if compare.ops.len() == 2 && compare.comparators.len() == 2 {
        if let Some(range_constraints) = eval_range_constraint(compare, env, false) {
            constraints.arg_constraints.extend(range_constraints);
            return;
        }
    }

    let op = &compare.ops[0];
    let left = &compare.left;
    let comparator = &compare.comparators[0];

    let left_val = eval_expr(left, env);
    let right_val = eval_expr(comparator, env);

    // len(split_result) vs integer
    if let AbstractValue::SplitLength {
        base_offset,
        pops_from_end,
    } = &left_val
    {
        if let Some(n) = expr_as_usize(comparator) {
            let offset = *base_offset + *pops_from_end;
            let constraint = match op {
                CmpOp::NotEq => Some(ArgumentCountConstraint::Exact(n + offset)),
                CmpOp::Lt => Some(ArgumentCountConstraint::Min(n + offset)),
                CmpOp::LtE => Some(ArgumentCountConstraint::Min(n + 1 + offset)),
                CmpOp::Gt => Some(ArgumentCountConstraint::Max(n + offset)),
                CmpOp::GtE if n > 0 => Some(ArgumentCountConstraint::Max(n - 1 + offset)),
                _ => None,
            };
            if let Some(c) = constraint {
                constraints.arg_constraints.push(c);
            }
            return;
        }

        // `len(bits) not in (2, 3, 4)` → valid counts are {2+offset, 3+offset, 4+offset}
        if matches!(op, CmpOp::NotIn) {
            if let Some(values) = extract_int_collection(comparator) {
                let offset = *base_offset + *pops_from_end;
                constraints
                    .arg_constraints
                    .push(ArgumentCountConstraint::OneOf(
                        values.into_iter().map(|v| v + offset).collect(),
                    ));
                return;
            }
        }
        return;
    }

    // Reversed: integer vs len(split_result), e.g. `4 < len(bits)`
    if let AbstractValue::SplitLength {
        base_offset,
        pops_from_end,
    } = &right_val
    {
        if let Some(n) = expr_as_usize(left) {
            let offset = *base_offset + *pops_from_end;
            let constraint = match op {
                CmpOp::Lt => Some(ArgumentCountConstraint::Max(n + offset)),
                CmpOp::LtE if n > 0 => Some(ArgumentCountConstraint::Max(n - 1 + offset)),
                CmpOp::Gt => Some(ArgumentCountConstraint::Min(n + offset)),
                CmpOp::GtE => Some(ArgumentCountConstraint::Min(n + 1 + offset)),
                _ => None,
            };
            if let Some(c) = constraint {
                constraints.arg_constraints.push(c);
            }
        }
        return;
    }

    // SplitElement vs string: `bits[N] != "keyword"`
    if let AbstractValue::SplitElement { index } = &left_val {
        if let Some(keyword) = extract_string_value(comparator) {
            let position = index_to_i64(index);
            constraints.required_keywords.push(RequiredKeyword {
                position,
                value: keyword,
            });
        }
        return;
    }

    // Reversed: string vs SplitElement: `"keyword" != bits[N]`
    if let AbstractValue::SplitElement { index } = &right_val {
        if let Some(keyword) = extract_string_value(left) {
            let position = index_to_i64(index);
            constraints.required_keywords.push(RequiredKeyword {
                position,
                value: keyword,
            });
        }
    }
}

fn eval_negated_compare(compare: &ExprCompare, env: &Env, constraints: &mut Constraints) {
    // Range: `not (2 <= len(bits) <= 4)` → valid range is min..=max
    if compare.ops.len() == 2 && compare.comparators.len() == 2 {
        if let Some(range_constraints) = eval_range_constraint(compare, env, true) {
            constraints.arg_constraints.extend(range_constraints);
            return;
        }
    }

    // Simple negation: `not len(bits) == 3` → Exact(3)
    if compare.ops.len() == 1 && compare.comparators.len() == 1 {
        let left_val = eval_expr(&compare.left, env);
        if let AbstractValue::SplitLength {
            base_offset,
            pops_from_end,
        } = left_val
        {
            if let Some(n) = expr_as_usize(&compare.comparators[0]) {
                let offset = base_offset + pops_from_end;
                let constraint = match &compare.ops[0] {
                    CmpOp::Eq => Some(ArgumentCountConstraint::Exact(n + offset)),
                    CmpOp::Lt if n > 0 => Some(ArgumentCountConstraint::Max(n - 1 + offset)),
                    CmpOp::Gt => Some(ArgumentCountConstraint::Min(n + 1 + offset)),
                    _ => None,
                };
                if let Some(c) = constraint {
                    constraints.arg_constraints.push(c);
                }
            }
        }
    }
}

/// Extract range constraint from `CONST <=/<  len(var) <=/<  CONST`.
fn eval_range_constraint(
    compare: &ExprCompare,
    env: &Env,
    negated: bool,
) -> Option<Vec<ArgumentCountConstraint>> {
    if compare.ops.len() != 2 || compare.comparators.len() != 2 {
        return None;
    }

    let middle = eval_expr(&compare.comparators[0], env);
    let AbstractValue::SplitLength {
        base_offset,
        pops_from_end,
    } = middle
    else {
        return None;
    };
    let base_offset = base_offset + pops_from_end;

    let lower = expr_as_usize(&compare.left)?;
    let upper = expr_as_usize(&compare.comparators[1])?;

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
        upper - 1
    };

    // Both negated and non-negated produce Min/Max bounds — the caller
    // already handles the inversion semantics based on error-guard context
    let _ = negated;
    Some(vec![
        ArgumentCountConstraint::Min(min_val + base_offset),
        ArgumentCountConstraint::Max(max_val + base_offset),
    ])
}

fn index_to_i64(index: &Index) -> i64 {
    match index {
        Index::Forward(n) => i64::try_from(*n).unwrap_or(0),
        Index::Backward(n) => -(i64::try_from(*n).unwrap_or(0)),
    }
}

fn expr_as_usize(expr: &Expr) -> Option<usize> {
    match expr {
        Expr::NumberLiteral(lit) => match &lit.value {
            Number::Int(int_val) => {
                let val = int_val.as_u64()?;
                usize::try_from(val).ok()
            }
            _ => None,
        },
        _ => None,
    }
}

fn extract_string_value(expr: &Expr) -> Option<String> {
    if let Expr::StringLiteral(ExprStringLiteral { value, .. }) = expr {
        return Some(value.to_str().to_string());
    }
    None
}

fn extract_int_collection(expr: &Expr) -> Option<Vec<usize>> {
    let elements = match expr {
        Expr::Tuple(t) => &t.elts,
        Expr::List(l) => &l.elts,
        Expr::Set(s) => &s.elts,
        _ => return None,
    };
    let mut values = Vec::new();
    for elt in elements {
        values.push(expr_as_usize(elt)?);
    }
    Some(values)
}

fn body_raises_template_syntax_error(body: &[Stmt]) -> bool {
    for stmt in body {
        if let Stmt::Raise(StmtRaise { exc: Some(exc), .. }) = stmt {
            if is_template_syntax_error_call(exc) {
                return true;
            }
        }
    }
    false
}

fn is_template_syntax_error_call(expr: &Expr) -> bool {
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
    use ruff_python_parser::parse_module;

    use super::*;
    use crate::dataflow::calls::HelperCache;
    use crate::dataflow::domain::Env;
    use crate::dataflow::eval::AnalysisContext;
    use crate::dataflow::eval::process_statements;

    fn extract_from_source(source: &str) -> Constraints {
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
        };
        process_statements(&func.body, &mut env, &mut ctx);
        extract_constraints(&func.body, &env)
    }

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
                position: 2,
                value: "as".to_string()
            }]
        );
    }

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
                position: -1,
                value: "silent".to_string()
            }]
        );
    }

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
                position: 1,
                value: "as".to_string()
            }]
        );
    }

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
                position: 2,
                value: "as".to_string()
            }]
        );
    }

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
                position: 2,
                value: "as".to_string()
            }]
        );
    }

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

    #[test]
    fn regroup_pattern_end_to_end() {
        let c = extract_from_source(
            r#"
def do_tag(parser, token):
    bits = token.split_contents()
    if len(bits) != 6:
        raise TemplateSyntaxError("err")
    if bits[2] != "by":
        raise TemplateSyntaxError("err")
    if bits[4] != "as":
        raise TemplateSyntaxError("err")
"#,
        );
        assert_eq!(c.arg_constraints, vec![ArgumentCountConstraint::Exact(6)]);
        assert_eq!(
            c.required_keywords,
            vec![
                RequiredKeyword {
                    position: 2,
                    value: "by".to_string()
                },
                RequiredKeyword {
                    position: 4,
                    value: "as".to_string()
                }
            ]
        );
    }

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
                position: 2,
                value: "as".to_string()
            }]
        );
    }

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
}

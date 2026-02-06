use ruff_python_ast::CmpOp;
use ruff_python_ast::Expr;
use ruff_python_ast::Stmt;

use crate::context::FunctionContext;
use crate::parser::ParsedModule;
use crate::patterns;
use crate::registry::RegistrationInfo;
use crate::types::ExtractedRule;
use crate::types::RuleCondition;
use crate::ExtractionError;

/// Extract validation rules from a tag function.
///
/// Uses the detected `split_var` name from `FunctionContext` to identify
/// relevant conditions (e.g., `len(bits)`, `args[1]`, etc.).
#[allow(clippy::unnecessary_wraps)]
pub fn extract_tag_rules(
    parsed: &ParsedModule,
    registration: &RegistrationInfo,
    ctx: &FunctionContext,
) -> Result<Vec<ExtractedRule>, ExtractionError> {
    let module = parsed.ast();

    let func_def = module.body.iter().find_map(|stmt| {
        if let Stmt::FunctionDef(fd) = stmt {
            if fd.name.as_str() == registration.function_name {
                return Some(fd);
            }
        }
        None
    });

    let Some(func_def) = func_def else {
        return Ok(Vec::new());
    };

    let Some(split_var) = ctx.split_var() else {
        return Ok(Vec::new());
    };

    let mut rules = Vec::new();
    extract_rules_from_stmts(&func_def.body, split_var, &mut rules);

    Ok(rules)
}

fn extract_rules_from_stmts(
    stmts: &[Stmt],
    split_var: &str,
    rules: &mut Vec<ExtractedRule>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::If(if_stmt) => {
                if has_template_syntax_error_raise(&if_stmt.body)
                    || if_stmt
                        .elif_else_clauses
                        .iter()
                        .any(|c| has_template_syntax_error_raise(&c.body))
                {
                    if let Some(condition) =
                        analyze_condition(&if_stmt.test, split_var)
                    {
                        let message =
                            extract_error_message(&if_stmt.body);
                        rules.push(ExtractedRule { condition, message });
                    }
                }

                extract_rules_from_stmts(
                    &if_stmt.body,
                    split_var,
                    rules,
                );
                for clause in &if_stmt.elif_else_clauses {
                    extract_rules_from_stmts(
                        &clause.body,
                        split_var,
                        rules,
                    );
                }
            }

            Stmt::While(while_stmt) => {
                extract_rules_from_stmts(
                    &while_stmt.body,
                    split_var,
                    rules,
                );
            }

            Stmt::For(for_stmt) => {
                extract_rules_from_stmts(
                    &for_stmt.body,
                    split_var,
                    rules,
                );
            }

            Stmt::Try(try_stmt) => {
                extract_rules_from_stmts(
                    &try_stmt.body,
                    split_var,
                    rules,
                );
            }

            _ => {}
        }
    }
}

fn has_template_syntax_error_raise(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|stmt| {
        if let Stmt::Raise(raise) = stmt {
            if let Some(exc) = &raise.exc {
                return is_template_syntax_error(exc);
            }
        }
        false
    })
}

fn is_template_syntax_error(expr: &Expr) -> bool {
    if let Expr::Call(call) = expr {
        match call.func.as_ref() {
            Expr::Name(name) => {
                name.id.as_str() == "TemplateSyntaxError"
            }
            Expr::Attribute(attr) => {
                attr.attr.as_str() == "TemplateSyntaxError"
            }
            _ => false,
        }
    } else {
        false
    }
}

fn extract_error_message(stmts: &[Stmt]) -> Option<String> {
    for stmt in stmts {
        if let Stmt::Raise(raise) = stmt {
            if let Some(exc) = &raise.exc {
                if let Expr::Call(call) = exc.as_ref() {
                    if let Some(Expr::StringLiteral(s)) =
                        call.arguments.args.first()
                    {
                        return Some(s.value.to_string());
                    }
                }
            }
        }
    }
    None
}

/// Analyze a condition expression using the detected split variable name.
fn analyze_condition(
    expr: &Expr,
    split_var: &str,
) -> Option<RuleCondition> {
    match expr {
        Expr::Compare(cmp) => analyze_comparison(cmp, split_var),

        Expr::UnaryOp(unary)
            if matches!(
                unary.op,
                ruff_python_ast::UnaryOp::Not
            ) =>
        {
            analyze_condition(&unary.operand, split_var)
                .map(negate_condition)
        }

        _ => Some(RuleCondition::Opaque {
            description: "complex condition".to_string(),
        }),
    }
}

fn analyze_comparison(
    cmp: &ruff_python_ast::ExprCompare,
    split_var: &str,
) -> Option<RuleCondition> {
    if cmp.ops.len() != 1 || cmp.comparators.len() != 1 {
        return Some(RuleCondition::Opaque {
            description: "compound comparison".to_string(),
        });
    }

    let op = &cmp.ops[0];
    let left = &cmp.left;
    let right = &cmp.comparators[0];

    // len(<split_var>) <op> N
    if patterns::is_len_call(left, split_var).is_some() {
        if let Some(n) = patterns::extract_int_literal(right) {
            return Some(len_op_to_condition(*op, n, false));
        }
    }

    // N <op> len(<split_var>) (reversed)
    if patterns::is_len_call(right, split_var).is_some() {
        if let Some(n) = patterns::extract_int_literal(left) {
            return Some(len_op_to_condition(*op, n, true));
        }
    }

    // <split_var>[N] == "keyword" or <split_var>[N] != "keyword"
    if let Some((idx, subscript_name)) =
        patterns::extract_subscript_index(left)
    {
        if subscript_name == split_var {
            if let Some(s) = patterns::extract_string_literal(right) {
                return Some(match op {
                    CmpOp::Eq => RuleCondition::LiteralAt {
                        index: idx,
                        value: s,
                        negated: false,
                    },
                    CmpOp::NotEq => RuleCondition::LiteralAt {
                        index: idx,
                        value: s,
                        negated: true,
                    },
                    _ => {
                        return None;
                    }
                });
            }
        }
    }

    // "keyword" in <split_var> or "keyword" not in <split_var>
    if matches!(op, CmpOp::In | CmpOp::NotIn) {
        if let Some(s) = patterns::extract_string_literal(left) {
            if patterns::is_name(right, split_var) {
                return Some(RuleCondition::ContainsLiteral {
                    value: s,
                    negated: matches!(op, CmpOp::NotIn),
                });
            }
        }
    }

    // <split_var>[N] in ("opt1", "opt2") or <split_var>[N] not in (...)
    if matches!(op, CmpOp::In | CmpOp::NotIn) {
        if let Some((idx, subscript_name)) =
            patterns::extract_subscript_index(left)
        {
            if subscript_name == split_var {
                if let Some(choices) =
                    patterns::extract_string_tuple(right)
                {
                    return Some(RuleCondition::ChoiceAt {
                        index: idx,
                        choices,
                        negated: matches!(op, CmpOp::NotIn),
                    });
                }
            }
        }
    }

    Some(RuleCondition::Opaque {
        description: "unrecognized comparison".to_string(),
    })
}

/// Convert a `len()` comparison to a `RuleCondition`.
///
/// When `reversed` is true, the literal is on the left side of the comparison
/// (e.g., `3 < len(bits)`), so operators need to be flipped.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn len_op_to_condition(
    op: CmpOp,
    n: i64,
    reversed: bool,
) -> RuleCondition {
    let effective_op = if reversed { flip_op(op) } else { op };

    match effective_op {
        CmpOp::Eq => RuleCondition::ExactArgCount {
            count: n as usize,
            negated: false,
        },
        CmpOp::NotEq => RuleCondition::ExactArgCount {
            count: n as usize,
            negated: true,
        },
        CmpOp::Lt => RuleCondition::MaxArgCount {
            max: (n - 1) as usize,
        },
        CmpOp::LtE => RuleCondition::MaxArgCount {
            max: n as usize,
        },
        CmpOp::Gt => RuleCondition::MinArgCount {
            min: (n + 1) as usize,
        },
        CmpOp::GtE => RuleCondition::MinArgCount {
            min: n as usize,
        },
        _ => RuleCondition::Opaque {
            description: "unsupported comparison operator".to_string(),
        },
    }
}

/// Flip a comparison operator for reversed comparisons.
///
/// When we see `3 < len(bits)`, we need to interpret it as `len(bits) > 3`.
const fn flip_op(op: CmpOp) -> CmpOp {
    match op {
        CmpOp::Lt => CmpOp::Gt,
        CmpOp::LtE => CmpOp::GtE,
        CmpOp::Gt => CmpOp::Lt,
        CmpOp::GtE => CmpOp::LtE,
        other => other,
    }
}

fn negate_condition(cond: RuleCondition) -> RuleCondition {
    match cond {
        RuleCondition::ExactArgCount { count, negated } => {
            RuleCondition::ExactArgCount {
                count,
                negated: !negated,
            }
        }
        RuleCondition::LiteralAt {
            index,
            value,
            negated,
        } => RuleCondition::LiteralAt {
            index,
            value,
            negated: !negated,
        },
        RuleCondition::ChoiceAt {
            index,
            choices,
            negated,
        } => RuleCondition::ChoiceAt {
            index,
            choices,
            negated: !negated,
        },
        RuleCondition::ContainsLiteral { value, negated } => {
            RuleCondition::ContainsLiteral {
                value,
                negated: !negated,
            }
        }
        other => other,
    }
}

#[cfg(test)]
#[allow(clippy::needless_raw_string_hashes)]
mod tests {
    use super::*;
    use crate::extract_rules;

    #[test]
    fn extract_with_bits() {
        let source = r#"
@register.tag
def my_tag(parser, token):
    bits = token.split_contents()
    if len(bits) != 3:
        raise TemplateSyntaxError("Need 2 args")
    return Node()
"#;
        let result = extract_rules(source).unwrap();
        assert_eq!(result.tags.len(), 1);
        assert_eq!(result.tags[0].rules.len(), 1);
        assert!(matches!(
            &result.tags[0].rules[0].condition,
            RuleCondition::ExactArgCount {
                count: 3,
                negated: true
            }
        ));
        assert_eq!(
            result.tags[0].rules[0].message.as_deref(),
            Some("Need 2 args")
        );
    }

    #[test]
    fn extract_with_args() {
        let source = r#"
@register.tag
def my_tag(parser, token):
    args = token.split_contents()
    if len(args) < 2:
        raise TemplateSyntaxError("Need at least 1 arg")
    return Node()
"#;
        let result = extract_rules(source).unwrap();
        assert_eq!(result.tags.len(), 1);
        assert_eq!(result.tags[0].rules.len(), 1);
        assert!(matches!(
            &result.tags[0].rules[0].condition,
            RuleCondition::MaxArgCount { max: 1 }
        ));
    }

    #[test]
    fn extract_with_parts() {
        let source = r#"
@register.tag
def my_tag(p, t):
    parts = t.split_contents()
    if parts[1] != "as":
        raise TemplateSyntaxError("Expected 'as'")
    return Node()
"#;
        let result = extract_rules(source).unwrap();
        assert_eq!(result.tags.len(), 1);
        assert_eq!(result.tags[0].rules.len(), 1);
        assert!(matches!(
            &result.tags[0].rules[0].condition,
            RuleCondition::LiteralAt {
                index: 1,
                ref value,
                negated: true
            } if value == "as"
        ));
    }

    #[test]
    fn extract_exact_arg_count_eq() {
        let source = r#"
@register.tag
def my_tag(parser, token):
    bits = token.split_contents()
    if len(bits) == 1:
        raise TemplateSyntaxError("No args")
    return Node()
"#;
        let result = extract_rules(source).unwrap();
        assert!(matches!(
            &result.tags[0].rules[0].condition,
            RuleCondition::ExactArgCount {
                count: 1,
                negated: false
            }
        ));
    }

    #[test]
    fn extract_min_arg_count() {
        let source = r#"
@register.tag
def my_tag(parser, token):
    bits = token.split_contents()
    if len(bits) > 4:
        raise TemplateSyntaxError("Too many args")
    return Node()
"#;
        let result = extract_rules(source).unwrap();
        assert!(matches!(
            &result.tags[0].rules[0].condition,
            RuleCondition::MinArgCount { min: 5 }
        ));
    }

    #[test]
    fn extract_max_arg_count_lte() {
        let source = r#"
@register.tag
def my_tag(parser, token):
    bits = token.split_contents()
    if len(bits) <= 2:
        raise TemplateSyntaxError("Need more args")
    return Node()
"#;
        let result = extract_rules(source).unwrap();
        assert!(matches!(
            &result.tags[0].rules[0].condition,
            RuleCondition::MaxArgCount { max: 2 }
        ));
    }

    #[test]
    fn extract_reversed_comparison() {
        let source = r#"
@register.tag
def my_tag(parser, token):
    bits = token.split_contents()
    if 3 < len(bits):
        raise TemplateSyntaxError("Too many args")
    return Node()
"#;
        let result = extract_rules(source).unwrap();
        // 3 < len(bits)  =>  len(bits) > 3  =>  MinArgCount { min: 4 }
        assert!(matches!(
            &result.tags[0].rules[0].condition,
            RuleCondition::MinArgCount { min: 4 }
        ));
    }

    #[test]
    fn extract_reversed_gte() {
        let source = r#"
@register.tag
def my_tag(parser, token):
    bits = token.split_contents()
    if 2 >= len(bits):
        raise TemplateSyntaxError("Need more")
    return Node()
"#;
        let result = extract_rules(source).unwrap();
        // 2 >= len(bits)  =>  len(bits) <= 2  =>  MaxArgCount { max: 2 }
        assert!(matches!(
            &result.tags[0].rules[0].condition,
            RuleCondition::MaxArgCount { max: 2 }
        ));
    }

    #[test]
    fn extract_contains_literal() {
        let source = r#"
@register.tag
def my_tag(parser, token):
    bits = token.split_contents()
    if "as" in bits:
        raise TemplateSyntaxError("'as' not allowed")
    return Node()
"#;
        let result = extract_rules(source).unwrap();
        assert!(matches!(
            &result.tags[0].rules[0].condition,
            RuleCondition::ContainsLiteral {
                ref value,
                negated: false
            } if value == "as"
        ));
    }

    #[test]
    fn extract_not_in_literal() {
        let source = r#"
@register.tag
def my_tag(parser, token):
    bits = token.split_contents()
    if "as" not in bits:
        raise TemplateSyntaxError("Missing 'as'")
    return Node()
"#;
        let result = extract_rules(source).unwrap();
        assert!(matches!(
            &result.tags[0].rules[0].condition,
            RuleCondition::ContainsLiteral {
                ref value,
                negated: true
            } if value == "as"
        ));
    }

    #[test]
    fn extract_choice_at() {
        let source = r#"
@register.tag
def my_tag(parser, token):
    bits = token.split_contents()
    if bits[1] in ("on", "off"):
        raise TemplateSyntaxError("Bad choice")
    return Node()
"#;
        let result = extract_rules(source).unwrap();
        assert!(matches!(
            &result.tags[0].rules[0].condition,
            RuleCondition::ChoiceAt {
                index: 1,
                ref choices,
                negated: false
            } if choices == &["on", "off"]
        ));
    }

    #[test]
    fn extract_not_unary_negation() {
        let source = r#"
@register.tag
def my_tag(parser, token):
    bits = token.split_contents()
    if not len(bits) == 3:
        raise TemplateSyntaxError("Need 2 args")
    return Node()
"#;
        let result = extract_rules(source).unwrap();
        // not (len(bits) == 3)  =>  ExactArgCount { count: 3, negated: true }
        assert!(matches!(
            &result.tags[0].rules[0].condition,
            RuleCondition::ExactArgCount {
                count: 3,
                negated: true
            }
        ));
    }

    #[test]
    fn extract_opaque_for_complex_condition() {
        let source = r#"
@register.tag
def my_tag(parser, token):
    bits = token.split_contents()
    if some_function(bits):
        raise TemplateSyntaxError("Complex")
    return Node()
"#;
        let result = extract_rules(source).unwrap();
        assert!(matches!(
            &result.tags[0].rules[0].condition,
            RuleCondition::Opaque { .. }
        ));
    }

    #[test]
    fn no_rules_without_split_contents() {
        let source = r#"
@register.simple_tag
def my_tag(value):
    return value
"#;
        let result = extract_rules(source).unwrap();
        assert_eq!(result.tags.len(), 1);
        assert!(result.tags[0].rules.is_empty());
    }

    #[test]
    fn multiple_rules_in_one_function() {
        let source = r#"
@register.tag
def my_tag(parser, token):
    bits = token.split_contents()
    if len(bits) < 2:
        raise TemplateSyntaxError("Too few")
    if len(bits) > 4:
        raise TemplateSyntaxError("Too many")
    return Node()
"#;
        let result = extract_rules(source).unwrap();
        assert_eq!(result.tags[0].rules.len(), 2);
        assert!(matches!(
            &result.tags[0].rules[0].condition,
            RuleCondition::MaxArgCount { max: 1 }
        ));
        assert!(matches!(
            &result.tags[0].rules[1].condition,
            RuleCondition::MinArgCount { min: 5 }
        ));
    }

    #[test]
    fn nested_rule_in_for_loop() {
        let source = r#"
@register.tag
def my_tag(parser, token):
    bits = token.split_contents()
    for item in bits:
        if len(bits) == 0:
            raise TemplateSyntaxError("Empty")
    return Node()
"#;
        let result = extract_rules(source).unwrap();
        assert_eq!(result.tags[0].rules.len(), 1);
        assert!(matches!(
            &result.tags[0].rules[0].condition,
            RuleCondition::ExactArgCount {
                count: 0,
                negated: false
            }
        ));
    }

    #[test]
    fn rule_in_try_block() {
        let source = r#"
@register.tag
def my_tag(parser, token):
    bits = token.split_contents()
    try:
        if len(bits) != 2:
            raise TemplateSyntaxError("Need 1 arg")
    except Exception:
        pass
    return Node()
"#;
        let result = extract_rules(source).unwrap();
        assert_eq!(result.tags[0].rules.len(), 1);
        assert!(matches!(
            &result.tags[0].rules[0].condition,
            RuleCondition::ExactArgCount {
                count: 2,
                negated: true
            }
        ));
    }

    #[test]
    fn error_message_with_format_string_not_extracted() {
        let source = r#"
@register.tag
def my_tag(parser, token):
    bits = token.split_contents()
    if len(bits) != 3:
        raise TemplateSyntaxError(f"Expected 2, got {len(bits)}")
    return Node()
"#;
        let result = extract_rules(source).unwrap();
        assert_eq!(result.tags[0].rules.len(), 1);
        // f-strings are not simple StringLiterals, so message is None
        assert!(result.tags[0].rules[0].message.is_none());
    }

    #[test]
    fn elif_with_raise() {
        let source = r#"
@register.tag
def my_tag(parser, token):
    bits = token.split_contents()
    if len(bits) == 2:
        pass
    elif len(bits) == 3:
        raise TemplateSyntaxError("Wrong count")
    return Node()
"#;
        let result = extract_rules(source).unwrap();
        // The condition checked is the if test (len(bits) == 2)
        // because has_template_syntax_error_raise checks elif bodies too
        assert_eq!(result.tags[0].rules.len(), 1);
    }
}

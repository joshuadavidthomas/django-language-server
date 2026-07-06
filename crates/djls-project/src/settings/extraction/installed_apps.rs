use ruff_python_ast as ast;

use crate::ast::ExprExt;
use crate::settings::extraction::INSTALLED_APPS;
use crate::settings::extraction::bindings::ExtractedList;
use crate::settings::extraction::bindings::ExtractedListStatus;
use crate::settings::extraction::env::EvalEnv;

pub(super) enum AssignmentEffect {
    Assign(ExtractedList<String>),
    Unsupported,
}

pub(super) fn evaluate_assignment(value: &ast::Expr, env: &EvalEnv<'_>) -> AssignmentEffect {
    match value {
        ast::Expr::List(_)
        | ast::Expr::Tuple(_)
        | ast::Expr::BinOp(ast::ExprBinOp {
            op: ast::Operator::Add,
            ..
        }) => AssignmentEffect::Assign(evaluate_list_operand(value, env)),
        expr if expr.name_target() == Some(INSTALLED_APPS) => {
            AssignmentEffect::Assign(evaluate_list_operand(value, env))
        }
        _ => AssignmentEffect::Unsupported,
    }
}

pub(super) fn evaluate_list_operand(value: &ast::Expr, env: &EvalEnv<'_>) -> ExtractedList<String> {
    match value {
        ast::Expr::List(list) => extract_string_elements(&list.elts),
        ast::Expr::Tuple(tuple) => extract_string_elements(&tuple.elts),
        ast::Expr::BinOp(bin_op) if bin_op.op == ast::Operator::Add => {
            let mut extracted = ExtractedList::complete(Vec::new());
            push_addition_values(value, env, &mut extracted);
            extracted
        }
        expr if expr.name_target() == Some(INSTALLED_APPS) => env.installed_apps().map_or_else(
            || ExtractedList::incomplete(Vec::new()),
            |setting| ExtractedList {
                values: setting.values.clone(),
                status: if setting.is_fully_extracted() {
                    ExtractedListStatus::Complete
                } else {
                    ExtractedListStatus::Incomplete
                },
            },
        ),
        _ => ExtractedList::incomplete(Vec::new()),
    }
}

fn push_addition_values(
    expr: &ast::Expr,
    env: &EvalEnv<'_>,
    extracted: &mut ExtractedList<String>,
) {
    match expr {
        ast::Expr::BinOp(bin_op) if bin_op.op == ast::Operator::Add => {
            push_addition_values(&bin_op.left, env, extracted);
            push_addition_values(&bin_op.right, env, extracted);
        }
        _ => {
            let operand = evaluate_list_operand(expr, env);
            extracted.values.extend(operand.values);
            if !operand.status.is_complete() {
                extracted.status.mark_incomplete();
            }
        }
    }
}

fn extract_string_elements(elements: &[ast::Expr]) -> ExtractedList<String> {
    let mut extracted = ExtractedList::complete(Vec::new());
    for element in elements {
        if let Some(value) = element.string_literal() {
            extracted.values.push(value.to_string());
        } else {
            extracted.status.mark_incomplete();
        }
    }
    extracted
}

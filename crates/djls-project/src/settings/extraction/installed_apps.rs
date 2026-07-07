use ruff_python_ast as ast;

use crate::ast::ExprExt;
use crate::settings::extraction::KnownSetting;
use crate::settings::extraction::bindings::ExtractedList;
use crate::settings::extraction::bindings::ExtractedListStatus;
use crate::settings::extraction::env::EvalEnv;
use crate::settings::types::LocalListBinding;

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
        expr if expr.name_target().is_some() => {
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
        expr => expr.name_target().map_or_else(
            || ExtractedList::incomplete(Vec::new()),
            |name| {
                if name == KnownSetting::InstalledApps.name() {
                    env.installed_apps().map_or_else(
                        || ExtractedList::incomplete(Vec::new()),
                        |setting| ExtractedList {
                            values: setting.values.clone(),
                            status: if setting.is_fully_extracted() {
                                ExtractedListStatus::Complete
                            } else {
                                ExtractedListStatus::Incomplete
                            },
                        },
                    )
                } else {
                    evaluate_known_name_operand(name, env)
                        .unwrap_or_else(|| ExtractedList::incomplete(Vec::new()))
                }
            },
        ),
    }
}

pub(super) fn evaluate_local_list_assignment(
    value: &ast::Expr,
    env: &EvalEnv<'_>,
) -> Option<ExtractedList<String>> {
    match value {
        ast::Expr::List(list) => extract_complete_string_elements(&list.elts),
        ast::Expr::Tuple(tuple) => extract_complete_string_elements(&tuple.elts),
        ast::Expr::BinOp(bin_op) if bin_op.op == ast::Operator::Add => {
            let mut extracted = ExtractedList::complete(Vec::new());
            push_known_addition_values(value, env, &mut extracted)?;
            Some(extracted)
        }
        expr => expr
            .name_target()
            .and_then(|name| evaluate_known_name_operand(name, env)),
    }
}

fn evaluate_known_name_operand(name: &str, env: &EvalEnv<'_>) -> Option<ExtractedList<String>> {
    if name == KnownSetting::InstalledApps.name() {
        return env.installed_apps().map(|setting| ExtractedList {
            values: setting.values.clone(),
            status: if setting.is_fully_extracted() {
                ExtractedListStatus::Complete
            } else {
                ExtractedListStatus::Incomplete
            },
        });
    }

    env.local_list_binding(name).map(|binding| ExtractedList {
        values: binding.values.clone(),
        status: if binding.is_fully_extracted() {
            ExtractedListStatus::Complete
        } else {
            ExtractedListStatus::Incomplete
        },
    })
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

fn push_known_addition_values(
    expr: &ast::Expr,
    env: &EvalEnv<'_>,
    extracted: &mut ExtractedList<String>,
) -> Option<()> {
    match expr {
        ast::Expr::BinOp(bin_op) if bin_op.op == ast::Operator::Add => {
            push_known_addition_values(&bin_op.left, env, extracted)?;
            push_known_addition_values(&bin_op.right, env, extracted)
        }
        _ => {
            let operand = evaluate_local_list_assignment(expr, env)?;
            extracted.values.extend(operand.values);
            if !operand.status.is_complete() {
                extracted.status.mark_incomplete();
            }
            Some(())
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

fn extract_complete_string_elements(elements: &[ast::Expr]) -> Option<ExtractedList<String>> {
    let extracted = extract_string_elements(elements);
    if extracted.status.is_complete() {
        Some(extracted)
    } else {
        None
    }
}

impl From<ExtractedList<String>> for LocalListBinding {
    fn from(value: ExtractedList<String>) -> Self {
        if value.status.is_complete() {
            Self::full(value.values)
        } else {
            Self::partial(value.values)
        }
    }
}

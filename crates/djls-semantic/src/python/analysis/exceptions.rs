use ruff_python_ast::Expr;
use ruff_python_ast::ExprBinOp;
use ruff_python_ast::ExprCall;
use ruff_python_ast::Operator;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtRaise;

use crate::python::analysis::expressions::eval_expr;
use crate::python::analysis::state::AbstractValue;
use crate::python::analysis::state::Env;
use djls_project::extraction::ExprExt;
use crate::python::types::ExtractedMessageArg;
use crate::python::types::ExtractedMessageTemplate;

/// Return the exception expression from the first direct `raise` in a statement body.
///
/// Only checks direct children (does not recurse into nested control flow).
/// Any exception type counts — `TemplateSyntaxError`, `ValueError`, etc.
pub(super) fn direct_raise_exception(body: &[Stmt]) -> Option<&Expr> {
    body.iter().find_map(|stmt| {
        let Stmt::Raise(StmtRaise { exc: Some(exc), .. }) = stmt else {
            return None;
        };
        Some(exc.as_ref())
    })
}

pub(super) fn extract_exception_message(
    expr: &Expr,
    env: &Env,
) -> Option<ExtractedMessageTemplate> {
    let Expr::Call(ExprCall { arguments, .. }) = expr else {
        return None;
    };
    let first_arg = arguments.args.first()?;

    if let Some(message) = first_arg.string_literal() {
        return Some(ExtractedMessageTemplate::Static(message));
    }

    let Expr::BinOp(ExprBinOp {
        left,
        op: Operator::Mod,
        right,
        ..
    }) = first_arg
    else {
        return None;
    };

    let template = left.string_literal()?;
    let args = match right.as_ref() {
        Expr::Tuple(tuple) => tuple
            .elts
            .iter()
            .map(|arg| extract_message_arg(arg, env))
            .collect::<Option<Vec<_>>>()?,
        arg => vec![extract_message_arg(arg, env)?],
    };

    Some(ExtractedMessageTemplate::PercentFormat { template, args })
}

fn extract_message_arg(expr: &Expr, env: &Env) -> Option<ExtractedMessageArg> {
    match eval_expr(expr, &mut env.clone()) {
        AbstractValue::SplitElement { index } => Some(ExtractedMessageArg::SplitElement(index)),
        AbstractValue::Str(value) => Some(ExtractedMessageArg::String(value)),
        AbstractValue::Int(value) => Some(ExtractedMessageArg::Int(value)),
        AbstractValue::Unknown
        | AbstractValue::Token
        | AbstractValue::Parser
        | AbstractValue::SplitResult(_)
        | AbstractValue::SplitLength(_)
        | AbstractValue::Tuple(_) => None,
    }
}

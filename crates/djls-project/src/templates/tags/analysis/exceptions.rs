use ruff_python_ast::Expr;
use ruff_python_ast::ExprBinOp;
use ruff_python_ast::ExprCall;
use ruff_python_ast::Operator;
use ruff_python_ast::Stmt;
use ruff_python_ast::StmtRaise;

use crate::ast::ExprExt;
use crate::templates::tags::analysis::expressions::eval_expr;
use crate::templates::tags::analysis::state::AbstractValue;
use crate::templates::tags::analysis::state::Env;
use crate::templates::tags::types::ExtractedMessageArg;
use crate::templates::tags::types::ExtractedMessageTemplate;

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
        return Some(ExtractedMessageTemplate::Static(message.to_string()));
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

    let template = left.string_literal()?.to_string();
    let args = match right.as_ref() {
        Expr::Tuple(tuple) => tuple
            .elts
            .iter()
            .map(|arg| extract_message_arg(arg, env))
            .collect::<Option<Vec<_>>>()?,
        arg @ (Expr::BoolOp(_)
        | Expr::Named(_)
        | Expr::BinOp(_)
        | Expr::UnaryOp(_)
        | Expr::Lambda(_)
        | Expr::If(_)
        | Expr::Dict(_)
        | Expr::Set(_)
        | Expr::ListComp(_)
        | Expr::SetComp(_)
        | Expr::DictComp(_)
        | Expr::Generator(_)
        | Expr::Await(_)
        | Expr::Yield(_)
        | Expr::YieldFrom(_)
        | Expr::Compare(_)
        | Expr::Call(_)
        | Expr::FString(_)
        | Expr::TString(_)
        | Expr::StringLiteral(_)
        | Expr::BytesLiteral(_)
        | Expr::NumberLiteral(_)
        | Expr::BooleanLiteral(_)
        | Expr::NoneLiteral(_)
        | Expr::EllipsisLiteral(_)
        | Expr::Attribute(_)
        | Expr::Subscript(_)
        | Expr::Starred(_)
        | Expr::Name(_)
        | Expr::List(_)
        | Expr::Slice(_)
        | Expr::IpyEscapeCommand(_)) => vec![extract_message_arg(arg, env)?],
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

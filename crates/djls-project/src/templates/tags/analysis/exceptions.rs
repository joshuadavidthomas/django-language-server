use ruff_python_ast::ConversionFlag;
use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprBinOp;
use ruff_python_ast::ExprCall;
use ruff_python_ast::ExprFString;
use ruff_python_ast::FStringPart;
use ruff_python_ast::InterpolatedStringElement;
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
pub(crate) fn direct_raise_exception(body: &[Stmt]) -> Option<&Expr> {
    body.iter().find_map(|stmt| {
        let Stmt::Raise(StmtRaise { exc: Some(exc), .. }) = stmt else {
            return None;
        };
        Some(exc.as_ref())
    })
}

pub(crate) fn extract_exception_message(
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
    if let AbstractValue::Str(message) = eval_expr(first_arg, &mut env.clone()) {
        return Some(ExtractedMessageTemplate::Static(message));
    }
    if let Expr::FString(fstring) = first_arg {
        return lower_fstring(fstring, env);
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

    let template = left.string_literal().map(str::to_string).or_else(|| {
        let AbstractValue::Str(value) = eval_expr(left, &mut env.clone()) else {
            return None;
        };
        Some(value)
    })?;
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

/// Lower an f-string to the `%` format it renders like. Format specs, `!a`, and `=` have no
/// `%` equivalent here, so they leave the message unextracted.
fn lower_fstring(fstring: &ExprFString, env: &Env) -> Option<ExtractedMessageTemplate> {
    let mut template = String::new();
    let mut args = Vec::new();
    for part in &fstring.value {
        let elements = match part {
            FStringPart::Literal(literal) => {
                template.push_str(&literal.value.replace('%', "%%"));
                continue;
            }
            FStringPart::FString(fstring) => &fstring.elements,
        };
        for element in elements {
            match element {
                InterpolatedStringElement::Literal(literal) => {
                    template.push_str(&literal.value.replace('%', "%%"));
                }
                InterpolatedStringElement::Interpolation(interpolation) => {
                    if interpolation.format_spec.is_some() || interpolation.debug_text.is_some() {
                        return None;
                    }
                    template.push_str(match interpolation.conversion {
                        ConversionFlag::None | ConversionFlag::Str => "%s",
                        ConversionFlag::Repr => "%r",
                        ConversionFlag::Ascii => return None,
                    });
                    args.push(extract_message_arg(&interpolation.expression, env)?);
                }
            }
        }
    }
    Some(if args.is_empty() {
        ExtractedMessageTemplate::Static(template.replace("%%", "%"))
    } else {
        ExtractedMessageTemplate::PercentFormat { template, args }
    })
}

/// `parser.origin.name`
fn is_origin_name(expr: &Expr, env: &Env) -> bool {
    let Expr::Attribute(ExprAttribute { value, attr, .. }) = expr else {
        return false;
    };
    let Expr::Attribute(ExprAttribute {
        value: parser,
        attr: origin,
        ..
    }) = value.as_ref()
    else {
        return false;
    };
    attr.as_str() == "name"
        && origin.as_str() == "origin"
        && matches!(eval_expr(parser, &mut env.clone()), AbstractValue::Parser)
}

fn extract_message_arg(expr: &Expr, env: &Env) -> Option<ExtractedMessageArg> {
    if is_origin_name(expr, env) {
        return Some(ExtractedMessageArg::OriginName);
    }
    if let Expr::Attribute(ExprAttribute { value, attr, .. }) = expr
        && attr.as_str() == "contents"
        && matches!(eval_expr(value, &mut env.clone()), AbstractValue::Token)
    {
        return Some(ExtractedMessageArg::TokenContents);
    }

    match eval_expr(expr, &mut env.clone()) {
        AbstractValue::SplitElement { index } => Some(ExtractedMessageArg::SplitElement(index)),
        AbstractValue::Str(value) => Some(ExtractedMessageArg::String(value)),
        AbstractValue::Int(value) => Some(ExtractedMessageArg::Int(value)),
        AbstractValue::Unknown
        | AbstractValue::Token
        | AbstractValue::Parser
        | AbstractValue::SplitResult(_)
        | AbstractValue::TagBits { .. }
        | AbstractValue::SplitLength(_)
        | AbstractValue::SplitPredicate(_)
        | AbstractValue::AssignmentMap(_)
        | AbstractValue::AssignmentRemainder(_)
        | AbstractValue::Tuple(_) => None,
    }
}

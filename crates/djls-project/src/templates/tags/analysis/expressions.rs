use ruff_python_ast::Arguments;
use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprCall;
use ruff_python_ast::ExprNumberLiteral;
use ruff_python_ast::ExprSlice;
use ruff_python_ast::ExprStringLiteral;
use ruff_python_ast::ExprSubscript;
use ruff_python_ast::ExprTuple;
use ruff_python_ast::Number;

use crate::ast::ExprExt;
use crate::templates::tags::analysis::CallContext;
use crate::templates::tags::analysis::calls::resolve_call;
use crate::templates::tags::analysis::state::AbstractValue;
use crate::templates::tags::analysis::state::Env;
use crate::templates::tags::analysis::state::TokenSplit;
use crate::templates::tags::types::SplitPosition;

/// Evaluate a Python expression against the abstract environment.
///
/// When `ctx` is provided, function calls can be resolved to module-local
/// helpers via bounded inlining.
pub(crate) fn eval_expr(expr: &Expr, env: &mut Env) -> AbstractValue {
    eval_expr_with_ctx(expr, env, None)
}

/// Evaluate a Python expression with optional analysis context for call resolution.
pub(super) fn eval_expr_with_ctx(
    expr: &Expr,
    env: &mut Env,
    ctx: Option<&mut CallContext<'_>>,
) -> AbstractValue {
    if let Some(name) = expr.name_target() {
        return env.get(name).clone();
    }

    match expr {
        Expr::NumberLiteral(ExprNumberLiteral {
            value: Number::Int(int_val),
            ..
        }) => int_val
            .as_i64()
            .map_or(AbstractValue::Unknown, AbstractValue::Int),

        Expr::StringLiteral(ExprStringLiteral { value, .. }) => {
            AbstractValue::Str(value.to_string())
        }

        Expr::Tuple(ExprTuple { elts, .. }) => {
            let mut ctx = ctx;
            let mut values = Vec::with_capacity(elts.len());
            for e in elts {
                values.push(eval_expr_with_ctx(e, env, ctx.as_deref_mut()));
            }
            AbstractValue::Tuple(values)
        }

        Expr::Call(call) => eval_call_with_ctx(call, env, ctx),

        Expr::Subscript(ExprSubscript { value, slice, .. }) => {
            let base = eval_expr_with_ctx(value, env, ctx);
            eval_subscript(&base, slice, env)
        }

        Expr::BoolOp(_)
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
        | Expr::FString(_)
        | Expr::TString(_)
        | Expr::BytesLiteral(_)
        | Expr::NumberLiteral(_)
        | Expr::BooleanLiteral(_)
        | Expr::NoneLiteral(_)
        | Expr::EllipsisLiteral(_)
        | Expr::Attribute(_)
        | Expr::Starred(_)
        | Expr::Name(_)
        | Expr::List(_)
        | Expr::Slice(_)
        | Expr::IpyEscapeCommand(_) => AbstractValue::Unknown,
    }
}

/// Evaluate a function/method call expression with optional context.
fn eval_call_with_ctx(
    call: &ExprCall,
    env: &mut Env,
    mut ctx: Option<&mut CallContext<'_>>,
) -> AbstractValue {
    if let Expr::Attribute(ExprAttribute { value, attr, .. }) = call.func.as_ref() {
        let obj = eval_expr_with_ctx(value, env, ctx.as_deref_mut());
        let method = attr.as_str();

        // token.split_contents()
        if matches!((&obj, method), (AbstractValue::Token, "split_contents")) {
            return AbstractValue::SplitResult(TokenSplit::fresh());
        }

        // parser.token.split_contents()
        if method == "split_contents"
            && let Expr::Attribute(ExprAttribute {
                value: inner_value,
                attr: inner_attr,
                ..
            }) = value.as_ref()
        {
            let inner_obj = eval_expr_with_ctx(inner_value, env, ctx.as_deref_mut());
            if matches!(inner_obj, AbstractValue::Parser) && inner_attr.as_str() == "token" {
                return AbstractValue::SplitResult(TokenSplit::fresh());
            }
        }

        // bits.pop(0) or bits.pop()
        if method == "pop" && matches!(obj, AbstractValue::SplitResult(_)) {
            return eval_pop_return(&obj, &call.arguments);
        }

        // token.contents.split(...)
        if method == "split"
            && let Expr::Attribute(ExprAttribute {
                value: inner_value,
                attr: inner_attr,
                ..
            }) = value.as_ref()
        {
            let inner_obj = eval_expr_with_ctx(inner_value, env, ctx.as_deref_mut());
            if matches!(inner_obj, AbstractValue::Token) && inner_attr.as_str() == "contents" {
                return eval_contents_split(&call.arguments);
            }
        }

        // Hardcoded external summaries for parser methods
        if matches!(obj, AbstractValue::Parser) {
            match method {
                "compile_filter" | "parse" | "delete_first_token" => {
                    return AbstractValue::Unknown;
                }
                _ => {}
            }
        }

        return AbstractValue::Unknown;
    }

    // Builtin calls: len(), list()
    if let Some(name) = call.func.name_target() {
        // len() and list() with single argument
        if let Some(arg) = call.arguments.args.first() {
            let val = eval_expr_with_ctx(arg, env, ctx.as_deref_mut());
            match name {
                "len" => {
                    if let AbstractValue::SplitResult(split) = val {
                        return AbstractValue::SplitLength(split);
                    }
                }
                "list" => {
                    if matches!(val, AbstractValue::SplitResult(_)) {
                        return val;
                    }
                }
                _ => {}
            }
        }

        // Hardcoded external summary: token_kwargs(bits, parser)
        // Mutates bits → mark it Unknown, return Unknown
        if name == "token_kwargs" {
            if let Some(arg_name) = call.arguments.args.first().and_then(ExprExt::name_target) {
                env.set(arg_name.to_string(), AbstractValue::Unknown);
            }
            return AbstractValue::Unknown;
        }

        // Try module-local function resolution
        if let Some(ctx) = ctx.as_mut() {
            let args: Vec<AbstractValue> = call
                .arguments
                .args
                .iter()
                .map(|a| eval_expr_with_ctx(a, env, Some(*ctx)))
                .collect();
            return resolve_call(name, &args, ctx);
        }
    }

    AbstractValue::Unknown
}

/// Handle `token.contents.split(...)` patterns.
fn eval_contents_split(args: &Arguments) -> AbstractValue {
    if args.args.is_empty() {
        return AbstractValue::SplitResult(TokenSplit::fresh());
    }

    // token.contents.split(None, 1) → Tuple of [SplitElement(Forward(0)), Unknown]
    if args.args.len() == 2
        && let Expr::NoneLiteral(_) = &args.args[0]
        && let Expr::NumberLiteral(ExprNumberLiteral {
            value: Number::Int(int_val),
            ..
        }) = &args.args[1]
        && int_val.as_i64() == Some(1)
    {
        return AbstractValue::Tuple(vec![
            AbstractValue::SplitElement {
                index: SplitPosition::Forward(0),
            },
            AbstractValue::Unknown,
        ]);
    }

    AbstractValue::SplitResult(TokenSplit::fresh())
}

/// Evaluate the return value of `split_result.pop(0)` or `split_result.pop()`.
///
/// This only computes the return value — the mutation of the split result
/// is handled in `process_pop_statement`.
fn eval_pop_return(obj: &AbstractValue, args: &Arguments) -> AbstractValue {
    let AbstractValue::SplitResult(split) = obj else {
        return AbstractValue::Unknown;
    };

    if let Some(arg) = args.args.first() {
        // bits.pop(0) — return element at front_offset
        if let Some(0) = arg.non_negative_integer() {
            return AbstractValue::SplitElement {
                index: split.resolve_index(0),
            };
        }
    } else {
        // bits.pop() — return last element (before pop)
        return AbstractValue::SplitElement {
            index: SplitPosition::Backward(split.back_offset() + 1),
        };
    }

    AbstractValue::Unknown
}

/// Convert an i64 to an `AbstractValue` index element based on sign.
///
/// Positive indices use `TokenSplit::resolve_index` to account for front offset.
/// Negative indices map directly to `SplitPosition::Backward`.
fn i64_to_index_element(n: i64, split: &TokenSplit) -> AbstractValue {
    if n >= 0 {
        let Ok(index) = usize::try_from(n) else {
            return AbstractValue::Unknown;
        };
        AbstractValue::SplitElement {
            index: split.resolve_index(index),
        }
    } else {
        let Ok(index) = usize::try_from(n.unsigned_abs()) else {
            return AbstractValue::Unknown;
        };
        AbstractValue::SplitElement {
            index: SplitPosition::Backward(index),
        }
    }
}

/// Evaluate subscript access on an abstract value.
fn eval_subscript(base: &AbstractValue, slice: &Expr, env: &mut Env) -> AbstractValue {
    let AbstractValue::SplitResult(split) = base else {
        return AbstractValue::Unknown;
    };

    // bits[N] or bits[-N]
    if let Expr::NumberLiteral(ExprNumberLiteral {
        value: Number::Int(int_val),
        ..
    }) = slice
    {
        return int_val
            .as_i64()
            .map_or(AbstractValue::Unknown, |n| i64_to_index_element(n, split));
    }

    // bits[unary -N]
    if let Expr::UnaryOp(unary) = slice
        && matches!(unary.op, ruff_python_ast::UnaryOp::USub)
    {
        if let Expr::NumberLiteral(ExprNumberLiteral {
            value: Number::Int(int_val),
            ..
        }) = unary.operand.as_ref()
            && let Some(n) = int_val.as_i64()
        {
            let Ok(index) = usize::try_from(n.unsigned_abs()) else {
                return AbstractValue::Unknown;
            };
            return AbstractValue::SplitElement {
                index: SplitPosition::Backward(index),
            };
        }
        return AbstractValue::Unknown;
    }

    // bits[N:], bits[:N], bits[:-N]
    if let Expr::Slice(ExprSlice {
        lower, upper, step, ..
    }) = slice
    {
        if step.is_some() {
            return AbstractValue::Unknown;
        }

        return match (lower.as_deref(), upper.as_deref()) {
            // bits[N:] — slice from N onwards
            (Some(lower_expr), None) => lower_expr
                .non_negative_integer()
                .map_or(AbstractValue::Unknown, |n| {
                    AbstractValue::SplitResult(split.after_slice_from(n))
                }),
            // bits[:N], bits[:-N], or bits[:] — truncation, preserve offset
            (None, _) => AbstractValue::SplitResult(*split),
            _ => AbstractValue::Unknown,
        };
    }

    // bits[variable]
    if slice.name_target().is_some()
        && let AbstractValue::Int(n) = eval_expr(slice, env)
    {
        return i64_to_index_element(n, split);
    }

    AbstractValue::Unknown
}

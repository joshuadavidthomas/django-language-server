use ruff_python_ast::Arguments;
use ruff_python_ast::CmpOp;
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
use crate::templates::tags::analysis::mutations::PopPosition;
use crate::templates::tags::analysis::state::AbstractValue;
use crate::templates::tags::analysis::state::Env;
use crate::templates::tags::analysis::state::SplitPredicate;
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

        Expr::Tuple(ExprTuple { elts, .. }) => eval_collection(elts, env, ctx),

        Expr::Attribute(attribute) => attribute
            .value
            .path_segments()
            .map(|mut path| {
                path.push(attribute.attr.to_string());
                path
            })
            .and_then(|path| env.get_static_path(&path).cloned())
            .unwrap_or(AbstractValue::Unknown),

        Expr::Call(call) => eval_call_with_ctx(call, env, ctx),

        Expr::Subscript(ExprSubscript { value, slice, .. }) => {
            let base = eval_expr_with_ctx(value, env, ctx);
            eval_subscript(&base, slice, env)
        }

        Expr::Compare(compare) => eval_split_equality(compare, env),

        Expr::UnaryOp(unary) if unary.op == ruff_python_ast::UnaryOp::USub => {
            match eval_expr(&unary.operand, env) {
                AbstractValue::Int(value) => value
                    .checked_neg()
                    .map_or(AbstractValue::Unknown, AbstractValue::Int),
                AbstractValue::Unknown
                | AbstractValue::Token
                | AbstractValue::Parser
                | AbstractValue::SplitResult(_)
                | AbstractValue::SplitElement { .. }
                | AbstractValue::SplitLength(_)
                | AbstractValue::Str(_)
                | AbstractValue::SplitPredicate(_)
                | AbstractValue::Tuple(_) => AbstractValue::Unknown,
            }
        }

        Expr::BinOp(binary) if binary.op == ruff_python_ast::Operator::Add => {
            match (eval_expr(&binary.left, env), eval_expr(&binary.right, env)) {
                (AbstractValue::Int(left), AbstractValue::Int(right)) => {
                    AbstractValue::Int(left + right)
                }
                _ => AbstractValue::Unknown,
            }
        }

        Expr::List(_)
        | Expr::Set(_)
        | Expr::BoolOp(_)
        | Expr::Named(_)
        | Expr::BinOp(_)
        | Expr::UnaryOp(_)
        | Expr::Lambda(_)
        | Expr::If(_)
        | Expr::Dict(_)
        | Expr::ListComp(_)
        | Expr::SetComp(_)
        | Expr::DictComp(_)
        | Expr::Generator(_)
        | Expr::Await(_)
        | Expr::Yield(_)
        | Expr::YieldFrom(_)
        | Expr::FString(_)
        | Expr::TString(_)
        | Expr::BytesLiteral(_)
        | Expr::NumberLiteral(_)
        | Expr::BooleanLiteral(_)
        | Expr::NoneLiteral(_)
        | Expr::EllipsisLiteral(_)
        | Expr::Starred(_)
        | Expr::Name(_)
        | Expr::Slice(_)
        | Expr::IpyEscapeCommand(_) => AbstractValue::Unknown,
    }
}

fn eval_collection(
    elements: &[Expr],
    env: &mut Env,
    mut ctx: Option<&mut CallContext<'_>>,
) -> AbstractValue {
    let mut values = Vec::with_capacity(elements.len());
    for element in elements {
        values.push(eval_expr_with_ctx(element, env, ctx.as_deref_mut()));
    }
    AbstractValue::Tuple(values)
}

/// A literal collection can be read at a membership test without keeping its
/// mutable contents as an exact value in the environment.
pub(super) fn eval_membership_collection(expr: &Expr, env: &mut Env) -> AbstractValue {
    if let Expr::List(list) = expr {
        eval_literal_collection(&list.elts, env, None)
    } else if let Expr::Set(set) = expr {
        eval_literal_collection(&set.elts, env, None)
    } else {
        eval_expr(expr, env)
    }
}

fn eval_literal_collection(
    elements: &[Expr],
    env: &mut Env,
    ctx: Option<&mut CallContext<'_>>,
) -> AbstractValue {
    if elements.is_empty() {
        return AbstractValue::Unknown;
    }
    let value = eval_collection(elements, env, ctx);
    let AbstractValue::Tuple(values) = &value else {
        return AbstractValue::Unknown;
    };
    if values
        .iter()
        .all(|value| matches!(value, AbstractValue::Int(_) | AbstractValue::Str(_)))
    {
        value
    } else {
        AbstractValue::Unknown
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

        if method == "pop"
            && let AbstractValue::SplitResult(split) = obj
        {
            return eval_pop(value, split, &call.arguments, env);
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
        // len() and list() with single argument. Bare names only identify
        // builtins when module and function scope analysis proves they are
        // visible.
        if env.builtin_name_visible(name)
            && let Some(arg) = call.arguments.args.first()
        {
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

fn eval_split_equality(compare: &ruff_python_ast::ExprCompare, env: &mut Env) -> AbstractValue {
    let [CmpOp::Eq] = &*compare.ops else {
        return AbstractValue::Unknown;
    };
    let [right] = &*compare.comparators else {
        return AbstractValue::Unknown;
    };
    match (eval_expr(&compare.left, env), eval_expr(right, env)) {
        (AbstractValue::SplitLength(split), AbstractValue::Int(length))
        | (AbstractValue::Int(length), AbstractValue::SplitLength(split)) => {
            usize::try_from(length)
                .ok()
                .map(|length| SplitPredicate::LengthEquals(split.resolve_length(length)))
                .map_or(AbstractValue::Unknown, AbstractValue::SplitPredicate)
        }
        (AbstractValue::SplitElement { index }, AbstractValue::Str(value))
        | (AbstractValue::Str(value), AbstractValue::SplitElement { index }) => {
            AbstractValue::SplitPredicate(SplitPredicate::ElementEquals {
                position: index,
                value,
            })
        }
        _ => AbstractValue::Unknown,
    }
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

fn eval_pop(receiver: &Expr, split: TokenSplit, args: &Arguments, env: &mut Env) -> AbstractValue {
    let (result, remaining) = match PopPosition::from_arguments(args) {
        PopPosition::Front => (
            AbstractValue::SplitElement {
                index: split.resolve_index(0),
            },
            AbstractValue::SplitResult(split.after_pop_front()),
        ),
        PopPosition::Back => (
            AbstractValue::SplitElement {
                index: SplitPosition::Backward(split.back_offset() + 1),
            },
            AbstractValue::SplitResult(split.after_pop_back()),
        ),
        PopPosition::Untracked => (AbstractValue::Unknown, AbstractValue::Unknown),
    };
    let original = AbstractValue::SplitResult(split);
    if let Some(name) = receiver.name_target() {
        // A later argument may have changed the receiver already.
        if env.get(name) != &original {
            env.forget_aliases(&original);
            return AbstractValue::Unknown;
        }
        env.mutate(name, |value| *value = remaining);
    } else {
        env.forget_aliases(&original);
    }
    result
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
    if let AbstractValue::Tuple(values) = base {
        let AbstractValue::Int(index) = eval_expr(slice, env) else {
            return AbstractValue::Unknown;
        };
        let index = if index < 0 {
            usize::try_from(index.unsigned_abs())
                .ok()
                .and_then(|distance| values.len().checked_sub(distance))
        } else {
            usize::try_from(index).ok()
        };
        return index
            .and_then(|index| values.get(index))
            .cloned()
            .unwrap_or(AbstractValue::Unknown);
    }

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
            // bits[N:-M] — retain the bounded middle of the original split.
            (Some(lower_expr), Some(upper_expr)) => {
                let Some(lower) = lower_expr.non_negative_integer() else {
                    return AbstractValue::Unknown;
                };
                let AbstractValue::Int(upper) = eval_expr(upper_expr, env) else {
                    return AbstractValue::Unknown;
                };
                if upper >= 0 {
                    return AbstractValue::Unknown;
                }
                let Ok(back) = usize::try_from(upper.unsigned_abs()) else {
                    return AbstractValue::Unknown;
                };
                let mut middle = split.after_slice_from(lower);
                for _ in 0..back {
                    middle = middle.after_pop_back();
                }
                AbstractValue::SplitResult(middle)
            }
            // bits[:N], bits[:-N], or bits[:] — truncation, preserve offset
            (None, _) => AbstractValue::SplitResult(*split),
        };
    }

    // bits[variable] or bits[tracked integer expression]
    if let AbstractValue::Int(n) = eval_expr(slice, env) {
        return i64_to_index_element(n, split);
    }

    AbstractValue::Unknown
}

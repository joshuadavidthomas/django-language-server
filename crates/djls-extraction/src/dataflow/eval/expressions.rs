use ruff_python_ast::Arguments;
use ruff_python_ast::Expr;
use ruff_python_ast::ExprAttribute;
use ruff_python_ast::ExprCall;
use ruff_python_ast::ExprName;
use ruff_python_ast::ExprNumberLiteral;
use ruff_python_ast::ExprSlice;
use ruff_python_ast::ExprStringLiteral;
use ruff_python_ast::ExprSubscript;
use ruff_python_ast::ExprTuple;
use ruff_python_ast::Number;

use super::AnalysisContext;
use crate::dataflow::calls::resolve_call;
use crate::dataflow::domain::AbstractValue;
use crate::dataflow::domain::Env;
use crate::types::SplitPosition;
use crate::ext::ExprExt;

/// Evaluate a Python expression against the abstract environment.
///
/// When `ctx` is provided, function calls can be resolved to module-local
/// helpers via bounded inlining.
pub fn eval_expr(expr: &Expr, env: &Env) -> AbstractValue {
    eval_expr_with_ctx(expr, env, None)
}

/// Evaluate a Python expression with optional analysis context for call resolution.
pub(super) fn eval_expr_with_ctx(
    expr: &Expr,
    env: &Env,
    ctx: Option<&mut AnalysisContext<'_>>,
) -> AbstractValue {
    match expr {
        Expr::Name(ExprName { id, .. }) => env.get(id.as_str()).clone(),

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

        _ => AbstractValue::Unknown,
    }
}

/// Evaluate a function/method call expression with optional context.
fn eval_call_with_ctx(
    call: &ExprCall,
    env: &Env,
    mut ctx: Option<&mut AnalysisContext<'_>>,
) -> AbstractValue {
    if let Expr::Attribute(ExprAttribute { value, attr, .. }) = call.func.as_ref() {
        let obj = eval_expr(value, env);
        let method = attr.as_str();

        // token.split_contents()
        if matches!((&obj, method), (AbstractValue::Token, "split_contents")) {
            return AbstractValue::SplitResult {
                base_offset: 0,
                pops_from_end: 0,
            };
        }

        // parser.token.split_contents()
        if method == "split_contents" {
            if let Expr::Attribute(ExprAttribute {
                value: inner_value,
                attr: inner_attr,
                ..
            }) = value.as_ref()
            {
                let inner_obj = eval_expr(inner_value, env);
                if matches!(inner_obj, AbstractValue::Parser) && inner_attr.as_str() == "token" {
                    return AbstractValue::SplitResult {
                        base_offset: 0,
                        pops_from_end: 0,
                    };
                }
            }
        }

        // bits.pop(0) or bits.pop()
        if method == "pop" && matches!(obj, AbstractValue::SplitResult { .. }) {
            return eval_pop_return(&obj, &call.arguments);
        }

        // token.contents.split(...)
        if method == "split" {
            if let Expr::Attribute(ExprAttribute {
                value: inner_value,
                attr: inner_attr,
                ..
            }) = value.as_ref()
            {
                let inner_obj = eval_expr(inner_value, env);
                if matches!(inner_obj, AbstractValue::Token) && inner_attr.as_str() == "contents" {
                    return eval_contents_split(&call.arguments);
                }
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
    if let Expr::Name(ExprName { id, .. }) = call.func.as_ref() {
        let name = id.as_str();

        // len() and list() with single argument
        if let Some(arg) = call.arguments.args.first() {
            let val = eval_expr(arg, env);
            match name {
                "len" => {
                    if let AbstractValue::SplitResult {
                        base_offset,
                        pops_from_end,
                    } = val
                    {
                        return AbstractValue::SplitLength {
                            base_offset,
                            pops_from_end,
                        };
                    }
                }
                "list" => {
                    if matches!(val, AbstractValue::SplitResult { .. }) {
                        return val;
                    }
                }
                _ => {}
            }
        }

        // Hardcoded external summary: token_kwargs(bits, parser)
        // Mutates bits → mark it Unknown, return Unknown
        if name == "token_kwargs" {
            return AbstractValue::Unknown;
        }

        // Try module-local function resolution
        if let Some(ctx) = ctx.as_mut() {
            let args: Vec<AbstractValue> = call
                .arguments
                .args
                .iter()
                .map(|a| eval_expr(a, env))
                .collect();
            return resolve_call(name, &args, ctx);
        }
    }

    AbstractValue::Unknown
}

/// Handle `token.contents.split(...)` patterns.
fn eval_contents_split(args: &Arguments) -> AbstractValue {
    if args.args.is_empty() {
        return AbstractValue::SplitResult {
            base_offset: 0,
            pops_from_end: 0,
        };
    }

    // token.contents.split(None, 1) → Tuple of [SplitElement(Forward(0)), Unknown]
    if args.args.len() == 2 {
        if let Expr::NoneLiteral(_) = &args.args[0] {
            if let Expr::NumberLiteral(ExprNumberLiteral {
                value: Number::Int(int_val),
                ..
            }) = &args.args[1]
            {
                if int_val.as_i64() == Some(1) {
                    return AbstractValue::Tuple(vec![
                        AbstractValue::SplitElement {
                            index: SplitPosition::Forward(0),
                        },
                        AbstractValue::Unknown,
                    ]);
                }
            }
        }
    }

    AbstractValue::SplitResult {
        base_offset: 0,
        pops_from_end: 0,
    }
}

/// Evaluate the return value of `split_result.pop(0)` or `split_result.pop()`.
///
/// This only computes the return value — the mutation of the split result
/// is handled in `process_pop_statement`.
fn eval_pop_return(obj: &AbstractValue, args: &Arguments) -> AbstractValue {
    let AbstractValue::SplitResult {
        base_offset,
        pops_from_end,
    } = obj
    else {
        return AbstractValue::Unknown;
    };

    if let Some(arg) = args.args.first() {
        // bits.pop(0) — return element at base_offset
        if let Some(0) = arg.positive_integer() {
            return AbstractValue::SplitElement {
                index: SplitPosition::Forward(*base_offset),
            };
        }
    } else {
        // bits.pop() — return last element (before pop)
        return AbstractValue::SplitElement {
            index: SplitPosition::Backward(*pops_from_end + 1),
        };
    }

    AbstractValue::Unknown
}

/// Convert an i64 to an `AbstractValue` index element based on sign.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn i64_to_index_element(n: i64, base_offset: usize) -> AbstractValue {
    if n >= 0 {
        AbstractValue::SplitElement {
            index: SplitPosition::Forward(base_offset + n as usize),
        }
    } else {
        AbstractValue::SplitElement {
            index: SplitPosition::Backward((-n) as usize),
        }
    }
}

/// Evaluate subscript access on an abstract value.
fn eval_subscript(base: &AbstractValue, slice: &Expr, env: &Env) -> AbstractValue {
    let AbstractValue::SplitResult { base_offset, .. } = base else {
        return AbstractValue::Unknown;
    };

    match slice {
        // bits[N] or bits[-N]
        Expr::NumberLiteral(ExprNumberLiteral {
            value: Number::Int(int_val),
            ..
        }) => int_val.as_i64().map_or(AbstractValue::Unknown, |n| {
            i64_to_index_element(n, *base_offset)
        }),

        // bits[unary -N]
        Expr::UnaryOp(unary) if matches!(unary.op, ruff_python_ast::UnaryOp::USub) => {
            if let Expr::NumberLiteral(ExprNumberLiteral {
                value: Number::Int(int_val),
                ..
            }) = unary.operand.as_ref()
            {
                if let Some(n) = int_val.as_i64() {
                    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                    return AbstractValue::SplitElement {
                        index: SplitPosition::Backward(n as usize),
                    };
                }
            }
            AbstractValue::Unknown
        }

        // bits[N:], bits[:N], bits[:-N]
        Expr::Slice(ExprSlice {
            lower, upper, step, ..
        }) => {
            if step.is_some() {
                return AbstractValue::Unknown;
            }

            let pops = if let AbstractValue::SplitResult { pops_from_end, .. } = base {
                *pops_from_end
            } else {
                0
            };
            match (lower.as_deref(), upper.as_deref()) {
                // bits[N:] — slice from N onwards
                (Some(lower_expr), None) => {
                    if let Some(n) = lower_expr.positive_integer() {
                        return AbstractValue::SplitResult {
                            base_offset: base_offset + n,
                            pops_from_end: pops,
                        };
                    }
                    AbstractValue::Unknown
                }
                // bits[:N], bits[:-N], or bits[:] — truncation, preserve offset
                (None, _) => AbstractValue::SplitResult {
                    base_offset: *base_offset,
                    pops_from_end: pops,
                },
                _ => AbstractValue::Unknown,
            }
        }

        // bits[variable]
        Expr::Name(_) => {
            let idx = eval_expr(slice, env);
            if let AbstractValue::Int(n) = idx {
                i64_to_index_element(n, *base_offset)
            } else {
                AbstractValue::Unknown
            }
        }

        _ => AbstractValue::Unknown,
    }
}

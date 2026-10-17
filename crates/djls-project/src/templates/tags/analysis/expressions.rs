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
use crate::templates::tags::analysis::TagSourceContext;
use crate::templates::tags::analysis::calls::evaluate_source_call;
use crate::templates::tags::analysis::mutations::PopPosition;
use crate::templates::tags::analysis::native;
use crate::templates::tags::analysis::state::AbstractValue;
use crate::templates::tags::analysis::state::Env;
use crate::templates::tags::analysis::state::SplitPredicate;
use crate::templates::tags::analysis::state::TagBitRead;
use crate::templates::tags::analysis::state::TokenSplit;
use crate::templates::tags::types::ArgumentConsumption;
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
    ctx: Option<&mut CallContext<'_, '_>>,
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
                | AbstractValue::TagBits { .. }
                | AbstractValue::SplitLength(_)
                | AbstractValue::Str(_)
                | AbstractValue::SplitPredicate(_)
                | AbstractValue::AssignmentMap(_)
                | AbstractValue::AssignmentRemainder(_)
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

        Expr::ListComp(comprehension) => {
            eval_split_comprehension(&comprehension.elt, &comprehension.generators, env, ctx)
        }
        Expr::Generator(generator) => {
            eval_split_comprehension(&generator.elt, &generator.generators, env, ctx)
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
        | Expr::SetComp(_)
        | Expr::DictComp(_)
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

/// Record the per-bit consumption of `[elt for bit in bits[...]]`.
///
/// The element expression runs once for every bit of the slice, so a use of
/// the loop variable holds for the whole slice, vacuously when it is empty.
/// Other facts depend on at least one iteration and are not kept.
fn eval_split_comprehension(
    element: &Expr,
    generators: &[ruff_python_ast::Comprehension],
    env: &mut Env,
    ctx: Option<&mut CallContext<'_, '_>>,
) -> AbstractValue {
    let [generator] = generators else {
        return AbstractValue::Unknown;
    };
    if generator.is_async || !generator.ifs.is_empty() {
        return AbstractValue::Unknown;
    }
    let Some(target) = generator.target.name_target() else {
        return AbstractValue::Unknown;
    };
    let AbstractValue::SplitResult(split) = eval_expr(&generator.iter, &mut env.clone()) else {
        return AbstractValue::Unknown;
    };
    let bits = split.span();
    let mut iteration = env.clone();
    iteration.set(
        target.to_string(),
        AbstractValue::TagBits {
            bits,
            read: TagBitRead::Bit,
        },
    );
    eval_expr_with_ctx(element, &mut iteration, ctx);
    if !iteration.bindings_match_except(env, target) {
        env.forget_split_results();
    }
    for consumed in iteration.consumed() {
        if consumed.bits == bits {
            env.consume(consumed.bits, consumed.consumption);
        }
    }
    AbstractValue::Unknown
}

/// Record that `value` is compiled as a filter expression.
fn consume_as_expression(value: &AbstractValue, env: &mut Env) {
    match value.read_bits() {
        Some((bits, TagBitRead::Bit)) => env.consume(bits, ArgumentConsumption::Expression),
        Some((bits, TagBitRead::KwargValue)) => {
            env.consume(bits, ArgumentConsumption::OptionalKeywordExpression);
        }
        Some((_, TagBitRead::KwargMatch)) | None => {}
    }
}

fn eval_collection(
    elements: &[Expr],
    env: &mut Env,
    mut ctx: Option<&mut CallContext<'_, '_>>,
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
    ctx: Option<&mut CallContext<'_, '_>>,
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
    mut ctx: Option<&mut CallContext<'_, '_>>,
) -> AbstractValue {
    let receiver = if let Expr::Attribute(attribute) = call.func.as_ref() {
        Some(eval_expr_with_ctx(
            &attribute.value,
            env,
            ctx.as_deref_mut(),
        ))
    } else {
        if call.func.name_target().is_none() {
            eval_expr_with_ctx(&call.func, env, ctx.as_deref_mut());
        }
        None
    };
    let args = call
        .arguments
        .args
        .iter()
        .map(|argument| {
            if let Expr::Starred(starred) = argument {
                let value = eval_expr_with_ctx(&starred.value, env, ctx.as_deref_mut());
                env.forget_aliases(&value);
                AbstractValue::Unknown
            } else {
                eval_expr_with_ctx(argument, env, ctx.as_deref_mut())
            }
        })
        .collect::<Vec<_>>();
    let keywords = call
        .arguments
        .keywords
        .iter()
        .map(|keyword| eval_expr_with_ctx(&keyword.value, env, ctx.as_deref_mut()))
        .collect::<Vec<_>>();

    if let (Expr::Attribute(ExprAttribute { value, attr, .. }), Some(obj)) =
        (call.func.as_ref(), receiver.as_ref())
    {
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
            return eval_pop(value, *split, &call.arguments, env);
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

        if let Some(value) = eval_method_summary(obj, method, &args, &keywords, env) {
            return value;
        }
    }

    if let Some(source) = ctx.as_deref_mut().and_then(|ctx| ctx.source.as_deref_mut())
        && let Some(value) = eval_django_filter_call(call, &args, &keywords, env, source)
    {
        return value;
    }

    if let Some(name @ ("len" | "list")) = call.func.name_target()
        && env.builtin_name_visible(name)
        && keywords.is_empty()
        && let [value] = args.as_slice()
    {
        return match (name, value) {
            ("len", AbstractValue::SplitResult(split)) => AbstractValue::SplitLength(*split),
            ("list", AbstractValue::SplitResult(split)) => AbstractValue::SplitResult(*split),
            _ => AbstractValue::Unknown,
        };
    }

    evaluate_source_call(call, &args, &keywords, receiver.as_ref(), env, ctx)
}

/// Hardcoded summaries for methods whose receiver the evaluator recognizes.
fn eval_method_summary(
    obj: &AbstractValue,
    method: &str,
    args: &[AbstractValue],
    keywords: &[AbstractValue],
    env: &mut Env,
) -> Option<AbstractValue> {
    match obj {
        AbstractValue::Parser => match method {
            "compile_filter" => {
                if let ([value], []) = (args, keywords) {
                    consume_as_expression(value, env);
                }
                Some(AbstractValue::Unknown)
            }
            "parse" | "delete_first_token" => Some(AbstractValue::Unknown),
            _ => None,
        },
        // The groups of a `kwarg_re` match: `(name, value)`.
        AbstractValue::TagBits {
            bits,
            read: TagBitRead::KwargMatch,
        } => {
            let value = AbstractValue::TagBits {
                bits: *bits,
                read: TagBitRead::KwargValue,
            };
            match (method, args, keywords) {
                ("groups", [], []) => {
                    Some(AbstractValue::Tuple(vec![AbstractValue::Unknown, value]))
                }
                ("group", [AbstractValue::Int(2)], []) => Some(value),
                _ => None,
            }
        }
        AbstractValue::Str(_) if method == "join" => Some(AbstractValue::Unknown),
        AbstractValue::Unknown
        | AbstractValue::Token
        | AbstractValue::SplitResult(_)
        | AbstractValue::SplitElement { .. }
        | AbstractValue::SplitLength(_)
        | AbstractValue::Int(_)
        | AbstractValue::Str(_)
        | AbstractValue::SplitPredicate(_)
        | AbstractValue::AssignmentMap(_)
        | AbstractValue::AssignmentRemainder(_)
        | AbstractValue::TagBits { .. }
        | AbstractValue::Tuple(_) => None,
    }
}

/// Calls to `django.template.base` members that read a Tag Bit like
/// `parser.compile_filter` does.
fn eval_django_filter_call(
    call: &ExprCall,
    args: &[AbstractValue],
    keywords: &[AbstractValue],
    env: &mut Env,
    source: &mut TagSourceContext<'_>,
) -> Option<AbstractValue> {
    // `kwarg_re.match(bit)` splits an optional `name=` prefix off the bit.
    if let Expr::Attribute(ExprAttribute { value, attr, .. }) = call.func.as_ref()
        && attr.as_str() == "match"
        && let ([argument], []) = (args, keywords)
        && let Some((bits, TagBitRead::Bit)) = argument.read_bits()
        && native::names_django_template_base_member(value, "kwarg_re", source)
    {
        return Some(AbstractValue::TagBits {
            bits,
            read: TagBitRead::KwargMatch,
        });
    }
    // `FilterExpression(bit, parser)` is what `parser.compile_filter` returns.
    // The name check only avoids resolving every call; identity decides.
    if let Some(token) = args.first()
        && call
            .func
            .path_segments()
            .is_some_and(|path| path.last().is_some_and(|name| name == "FilterExpression"))
        && native::names_django_template_base_member(&call.func, "FilterExpression", source)
    {
        consume_as_expression(token, env);
        return Some(AbstractValue::Unknown);
    }
    None
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
                index: split.resolve_backward_index(1),
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
/// Both directions resolve through the split so earlier front and back
/// removals keep pointing at the original token position.
fn i64_to_index_element(n: i64, split: &TokenSplit) -> AbstractValue {
    let Ok(index) = usize::try_from(n.unsigned_abs()) else {
        return AbstractValue::Unknown;
    };
    let index = if n >= 0 {
        split.resolve_index(index)
    } else {
        split.resolve_backward_index(index)
    };
    AbstractValue::SplitElement { index }
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

    // bits[N:], bits[:-M], bits[N:-M], or bits[:]
    if let Expr::Slice(ExprSlice {
        lower, upper, step, ..
    }) = slice
    {
        if step.is_some() {
            return AbstractValue::Unknown;
        }
        let lower = match lower.as_deref() {
            Some(lower) => lower.non_negative_integer(),
            None => Some(0),
        };
        let Some(lower) = lower else {
            return AbstractValue::Unknown;
        };
        let mut sliced = split.after_slice_from(lower);
        if let Some(upper) = upper.as_deref() {
            // A non-negative stop keeps a prefix whose length depends on the
            // invocation, so neither `len()` nor negative indexes stay exact.
            let AbstractValue::Int(upper) = eval_expr(upper, env) else {
                return AbstractValue::Unknown;
            };
            if upper >= 0 {
                return AbstractValue::Unknown;
            }
            let Ok(back) = usize::try_from(upper.unsigned_abs()) else {
                return AbstractValue::Unknown;
            };
            sliced = sliced.after_slice_back(back);
        }
        return AbstractValue::SplitResult(sliced);
    }

    // bits[variable] or bits[tracked integer expression]
    if let AbstractValue::Int(n) = eval_expr(slice, env) {
        return i64_to_index_element(n, split);
    }

    AbstractValue::Unknown
}

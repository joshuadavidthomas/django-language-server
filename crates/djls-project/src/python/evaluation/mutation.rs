use djls_source::Origin;
use ruff_python_ast as ast;

use super::PythonDictItem;
use super::PythonList;
use super::PythonListItem;
use super::PythonUnknownCause;
use super::PythonValue;
use super::PythonValueKind;
use super::evaluator::EvaluationContext;
use super::evaluator::EvaluationState;
use super::evaluator::expression::evaluate_value;
use super::touched_names::expr_read_names;
use crate::ast::ExprExt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonMutation {
    pub(crate) binding: String,
    pub(crate) path: Vec<PythonMutationPathSegment>,
    pub(crate) operation: PythonMutationOperation,
    pub(crate) origin: Origin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PythonMutationPathSegment {
    Index(usize),
    Key(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PythonMutationOperation {
    Append,
    Extend,
    Insert,
    Remove,
}

impl PythonMutationOperation {
    fn from_method(method: &str) -> Option<Self> {
        match method {
            "append" => Some(Self::Append),
            "extend" => Some(Self::Extend),
            "insert" => Some(Self::Insert),
            "remove" => Some(Self::Remove),
            _ => None,
        }
    }
}

pub(super) struct MutationTarget<'a> {
    pub(super) binding: &'a str,
    path: Vec<PythonMutationPathSegment>,
}

impl<'a> MutationTarget<'a> {
    pub(super) fn from_expr(expr: &'a ast::Expr) -> Option<Self> {
        let mut path = Vec::new();
        let binding = collect_mutation_target(expr, &mut path)?;
        path.reverse();
        Some(Self { binding, path })
    }

    pub(super) fn into_fact(
        self,
        operation: PythonMutationOperation,
        origin: Origin,
    ) -> PythonMutation {
        PythonMutation {
            binding: self.binding.to_string(),
            path: self.path,
            operation,
            origin,
        }
    }
}

fn collect_mutation_target<'a>(
    expr: &'a ast::Expr,
    path: &mut Vec<PythonMutationPathSegment>,
) -> Option<&'a str> {
    if let Some(name) = expr.name_target() {
        return Some(name);
    }

    let ast::Expr::Subscript(subscript) = expr else {
        return None;
    };

    if let Some(index) = subscript.slice.non_negative_integer() {
        path.push(PythonMutationPathSegment::Index(index));
    } else if let Some(key) = subscript.slice.string_literal() {
        path.push(PythonMutationPathSegment::Key(key.to_string()));
    } else {
        return None;
    }

    collect_mutation_target(&subscript.value, path)
}

pub(super) fn apply_augmented_add(
    state: &mut EvaluationState,
    target: MutationTarget<'_>,
    extension: &PythonValue,
    origin: Origin,
) {
    let supported = mutate_target(state, &target, origin, |value| {
        extend_list_value(value, extension, origin)
    });
    let binding = target.binding.to_string();
    state
        .mutations
        .push(target.into_fact(PythonMutationOperation::Extend, origin));
    if !supported {
        state.degrade_names([binding], &PythonUnknownCause::UnsupportedMutation, origin);
    }
}

pub(super) fn walk_expr(
    context: &EvaluationContext<'_>,
    state: &mut EvaluationState,
    expression: &ast::Expr,
) {
    let ast::Expr::Call(call) = expression else {
        state.degrade_names(
            expr_read_names(expression),
            &PythonUnknownCause::UnsupportedExpression,
            context.origin(expression),
        );
        return;
    };
    let ast::Expr::Attribute(attribute) = call.func.as_ref() else {
        state.degrade_names(
            expr_read_names(expression),
            &PythonUnknownCause::UnsupportedMutation,
            context.origin(expression),
        );
        return;
    };
    let Some(target) = MutationTarget::from_expr(&attribute.value) else {
        state.degrade_names(
            expr_read_names(expression),
            &PythonUnknownCause::UnsupportedMutation,
            context.origin(expression),
        );
        return;
    };
    let origin = context.origin(call);
    let Some(operation) = PythonMutationOperation::from_method(attribute.attr.as_str()) else {
        state.invalidate_names(
            [target.binding.to_string()],
            &PythonUnknownCause::UnsupportedMutation,
            origin,
        );
        return;
    };
    let supported = apply_mutation_operation(context, state, call, &target, operation, origin);
    if supported {
        state.mutations.push(target.into_fact(operation, origin));
    } else {
        state.invalidate_names(
            [target.binding.to_string()],
            &PythonUnknownCause::UnsupportedMutation,
            origin,
        );
    }
}

fn apply_mutation_operation(
    context: &EvaluationContext<'_>,
    state: &mut EvaluationState,
    call: &ast::ExprCall,
    target: &MutationTarget<'_>,
    operation: PythonMutationOperation,
    origin: Origin,
) -> bool {
    let arguments = call
        .arguments
        .args
        .iter()
        .map(|argument| evaluate_value(context, state, argument))
        .collect::<Vec<_>>();
    match (operation, arguments.as_slice()) {
        (PythonMutationOperation::Append, [argument]) => {
            mutate_target(state, target, origin, |value| {
                let PythonValueKind::List(list) = &mut value.kind else {
                    return false;
                };
                list.append(&PythonListItem::Value(argument.clone()));
                true
            })
        }
        (PythonMutationOperation::Extend, [extension]) => {
            mutate_target(state, target, origin, |value| {
                extend_list_value(value, extension, origin)
            })
        }
        (PythonMutationOperation::Insert, [_, argument]) => {
            let index = &call.arguments.args[0];
            let non_negative = index.non_negative_integer();
            let negative = index.negative_integer();
            (non_negative.is_some() || negative.is_some())
                && mutate_target(state, target, origin, |value| {
                    let PythonValueKind::List(list) = &mut value.kind else {
                        return false;
                    };
                    if !list_is_authoritative(list) {
                        return false;
                    }
                    let index = non_negative.map_or_else(
                        || {
                            let magnitude = negative.expect("insert index is an integer literal");
                            if magnitude == 0 {
                                0
                            } else {
                                list.items.len().saturating_sub(magnitude)
                            }
                        },
                        |index| index.min(list.items.len()),
                    );
                    list.insert(index, &PythonListItem::Value(argument.clone()));
                    true
                })
        }
        (PythonMutationOperation::Remove, [argument]) => {
            mutate_target(state, target, origin, |value| {
                let PythonValueKind::Str(needle) = &argument.kind else {
                    return false;
                };
                let PythonValueKind::List(list) = &mut value.kind else {
                    return false;
                };
                if !list_is_authoritative(list) {
                    return false;
                }
                let Some(index) = list.items.iter().position(|item| {
                    matches!(item, PythonListItem::Value(PythonValue {
                        kind: PythonValueKind::Str(candidate), ..
                    }) if candidate == needle)
                }) else {
                    return false;
                };
                list.remove(index);
                true
            })
        }
        (
            PythonMutationOperation::Append
            | PythonMutationOperation::Extend
            | PythonMutationOperation::Insert
            | PythonMutationOperation::Remove,
            _,
        ) => false,
    }
}

fn extend_list_value(value: &mut PythonValue, extension: &PythonValue, origin: Origin) -> bool {
    let PythonValueKind::List(list) = &mut value.kind else {
        return false;
    };
    match &extension.kind {
        PythonValueKind::List(extension) => list.extend(extension, origin),
        PythonValueKind::Unknown(unknown) => {
            list.append(&PythonListItem::UnknownUnpack(unknown.clone()));
        }
        PythonValueKind::Str(_)
        | PythonValueKind::Bool(_)
        | PythonValueKind::Path(_)
        | PythonValueKind::Dict(_) => return false,
    }
    true
}

fn list_is_authoritative(list: &PythonList) -> bool {
    list.items
        .iter()
        .all(|item| matches!(item, PythonListItem::Value(_)))
}

fn mutate_target(
    state: &mut EvaluationState,
    target: &MutationTarget<'_>,
    origin: Origin,
    mutate: impl Fn(&mut PythonValue) -> bool,
) -> bool {
    let Some(binding) = state.bindings.get_mut(target.binding) else {
        return false;
    };
    let Some(bound) = binding.single_bound_mut() else {
        return false;
    };
    let supported = mutate_at_path(&mut bound.value, &target.path, origin, &mutate);
    if supported {
        bound.value.normalize();
    }
    supported
}

fn mutate_at_path(
    value: &mut PythonValue,
    path: &[PythonMutationPathSegment],
    origin: Origin,
    mutate: &impl Fn(&mut PythonValue) -> bool,
) -> bool {
    let Some((next_segment, remaining)) = path.split_first() else {
        if !mutate(value) {
            return false;
        }
        value.record_origin(origin);
        value.normalize();
        return true;
    };

    match next_segment {
        PythonMutationPathSegment::Index(index) => {
            let PythonValueKind::List(list) = &mut value.kind else {
                return false;
            };
            let Some(PythonListItem::Value(next)) = list.items.get_mut(*index) else {
                return false;
            };
            if !mutate_at_path(next, remaining, origin, mutate) {
                return false;
            }
            for variant in &mut list.variants {
                if super::value::is_list_variant_limit_unknown(&variant.items) {
                    continue;
                }
                let Some(PythonListItem::Value(next)) = variant.items.get_mut(*index) else {
                    return false;
                };
                if !mutate_at_path(next, remaining, origin, mutate) {
                    return false;
                }
            }
            true
        }
        PythonMutationPathSegment::Key(key) => {
            let PythonValueKind::Dict(dict) = &mut value.kind else {
                return false;
            };
            let Some(next) = dict.items.iter_mut().rev().find_map(|item| match item {
                PythonDictItem::Entry { key: candidate, value }
                    if matches!(&candidate.kind, PythonValueKind::Str(candidate) if candidate == key) =>
                {
                    Some(value)
                }
                PythonDictItem::Entry { .. } | PythonDictItem::UnknownUnpack(_) => None,
            }) else {
                return false;
            };
            mutate_at_path(next, remaining, origin, mutate)
        }
    }
}

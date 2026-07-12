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
use super::evaluator::evaluate_value;
use super::touched_names::expr_read_names;
use crate::ast::ExprExt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonMutation {
    pub(crate) root: String,
    pub(crate) access: Vec<PythonMutationAccess>,
    pub(crate) method: String,
    pub(crate) origin: Origin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PythonMutationAccess {
    Index(usize),
    Key(String),
}

pub(super) struct MutationTarget<'a> {
    pub(super) root: &'a str,
    access: Vec<MutationAccess>,
}

impl<'a> MutationTarget<'a> {
    pub(super) fn from_expr(expr: &'a ast::Expr) -> Option<Self> {
        let mut access = Vec::new();
        let root = collect_mutation_target(expr, &mut access)?;
        access.reverse();
        Some(Self { root, access })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum MutationAccess {
    Index(usize),
    Key(String),
}

fn collect_mutation_target<'a>(
    expr: &'a ast::Expr,
    access: &mut Vec<MutationAccess>,
) -> Option<&'a str> {
    if let Some(name) = expr.name_target() {
        return Some(name);
    }

    let ast::Expr::Subscript(subscript) = expr else {
        return None;
    };

    if let Some(index) = subscript.slice.non_negative_integer() {
        access.push(MutationAccess::Index(index));
    } else if let Some(key) = subscript.slice.string_literal() {
        access.push(MutationAccess::Key(key.to_string()));
    } else {
        return None;
    }

    collect_mutation_target(&subscript.value, access)
}

pub(super) fn apply_augmented_add(
    state: &mut EvaluationState,
    target: &MutationTarget<'_>,
    extension: &PythonValue,
    origin: Origin,
) {
    let supported = mutate_target(state, target, origin, |value| {
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
    });
    state.mutations.push(PythonMutation {
        root: target.root.to_string(),
        access: target.access.iter().map(access_to_public).collect(),
        method: "extend".to_string(),
        origin,
    });
    if !supported {
        state.degrade_names(
            [target.root.to_string()],
            PythonUnknownCause::UnsupportedMutation,
            origin,
        );
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
            PythonUnknownCause::UnsupportedExpression,
            context.origin(expression),
        );
        return;
    };
    let ast::Expr::Attribute(attribute) = call.func.as_ref() else {
        state.degrade_names(
            expr_read_names(expression),
            PythonUnknownCause::UnsupportedMutation,
            context.origin(expression),
        );
        return;
    };
    let Some(target) = MutationTarget::from_expr(&attribute.value) else {
        state.degrade_names(
            expr_read_names(expression),
            PythonUnknownCause::UnsupportedMutation,
            context.origin(expression),
        );
        return;
    };
    let method = attribute.attr.as_str();
    let origin = context.origin(call);
    let supported = apply_mutation_call(context, state, call, &target, method, origin);
    state.mutations.push(PythonMutation {
        root: target.root.to_string(),
        access: target.access.iter().map(access_to_public).collect(),
        method: method.to_string(),
        origin,
    });
    if !supported {
        state.invalidate_names(
            [target.root.to_string()],
            PythonUnknownCause::UnsupportedMutation,
            origin,
        );
    }
}

fn apply_mutation_call(
    context: &EvaluationContext<'_>,
    state: &mut EvaluationState,
    call: &ast::ExprCall,
    target: &MutationTarget<'_>,
    method: &str,
    origin: Origin,
) -> bool {
    let arguments = call
        .arguments
        .args
        .iter()
        .map(|argument| evaluate_value(context, state, argument))
        .collect::<Vec<_>>();
    match (method, arguments.as_slice()) {
        ("append", [argument]) => mutate_target(state, target, origin, |value| {
            let PythonValueKind::List(list) = &mut value.kind else {
                return false;
            };
            list.append(&PythonListItem::Value(argument.clone()));
            true
        }),
        ("extend", [extension]) => mutate_target(state, target, origin, |value| {
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
        }),
        ("insert", [_, argument]) => {
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
        ("remove", [argument]) => mutate_target(state, target, origin, |value| {
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
        }),
        ("append" | "extend" | "insert" | "remove" | _, _) => false,
    }
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
    let Some(binding) = state.bindings.0.get_mut(target.root) else {
        return false;
    };
    let Some(bound) = binding.single_bound_mut() else {
        return false;
    };
    let supported = mutate_at_access(&mut bound.value, &target.access, origin, &mutate);
    if supported {
        bound.value.normalize();
    }
    supported
}

fn mutate_at_access(
    value: &mut PythonValue,
    access: &[MutationAccess],
    origin: Origin,
    mutate: &impl Fn(&mut PythonValue) -> bool,
) -> bool {
    let Some((next_access, remaining)) = access.split_first() else {
        if !mutate(value) {
            return false;
        }
        value.record_origin(origin);
        value.normalize();
        return true;
    };

    match next_access {
        MutationAccess::Index(index) => {
            let PythonValueKind::List(list) = &mut value.kind else {
                return false;
            };
            let Some(PythonListItem::Value(next)) = list.items.get_mut(*index) else {
                return false;
            };
            if !mutate_at_access(next, remaining, origin, mutate) {
                return false;
            }
            for variant in &mut list.variants {
                if super::value::is_list_variant_limit_unknown(&variant.items) {
                    continue;
                }
                let Some(PythonListItem::Value(next)) = variant.items.get_mut(*index) else {
                    return false;
                };
                if !mutate_at_access(next, remaining, origin, mutate) {
                    return false;
                }
            }
            true
        }
        MutationAccess::Key(key) => {
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
            mutate_at_access(next, remaining, origin, mutate)
        }
    }
}

fn access_to_public(access: &MutationAccess) -> PythonMutationAccess {
    match access {
        MutationAccess::Index(index) => PythonMutationAccess::Index(*index),
        MutationAccess::Key(key) => PythonMutationAccess::Key(key.clone()),
    }
}

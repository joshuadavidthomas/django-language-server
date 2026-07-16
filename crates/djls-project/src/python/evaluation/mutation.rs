use djls_source::Origin;
use ruff_python_ast as ast;

use super::PythonDictItem;
use super::PythonList;
use super::PythonListItem;
use super::PythonUnknownCause;
use super::PythonValue;
use super::PythonValueKind;
use super::evaluator::EvaluationState;
use super::evaluator::Evaluator;
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

#[derive(Debug, Clone, Copy)]
enum InsertIndex {
    NonNegative(usize),
    Negative(usize),
}

enum EvaluatedMutation<'a> {
    Append(&'a PythonValue),
    Extend(&'a PythonValue),
    Insert {
        index: InsertIndex,
        value: &'a PythonValue,
    },
    Remove(&'a str),
}

impl EvaluatedMutation<'_> {
    fn apply(&self, value: &mut PythonValue, origin: Origin) -> bool {
        if !value.is_mutable_container() {
            return false;
        }
        match self {
            Self::Append(argument) => {
                let PythonValueKind::List(list) = &mut value.kind else {
                    return false;
                };
                list.append(&PythonListItem::Value((*argument).clone()));
            }
            Self::Extend(extension) => {
                return extend_list_value(value, extension, origin);
            }
            Self::Insert { index, value: item } => {
                let PythonValueKind::List(list) = &mut value.kind else {
                    return false;
                };
                if !list_is_authoritative(list) {
                    return false;
                }
                let len = list.semantic_items().len();
                let index = match index {
                    InsertIndex::NonNegative(index) => (*index).min(len),
                    InsertIndex::Negative(magnitude) => {
                        if *magnitude == 0 {
                            0
                        } else {
                            len.saturating_sub(*magnitude)
                        }
                    }
                };
                list.insert(index, &PythonListItem::Value((*item).clone()));
            }
            Self::Remove(needle) => {
                let PythonValueKind::List(list) = &mut value.kind else {
                    return false;
                };
                if !list_is_authoritative(list) {
                    return false;
                }
                let Some(index) = list.semantic_items().iter().position(|item| {
                    matches!(item, PythonListItem::Value(PythonValue {
                        kind: PythonValueKind::Str(candidate), ..
                    }) if candidate == needle)
                }) else {
                    return false;
                };
                list.remove(index);
            }
        }
        true
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
    let binding = target.binding.to_string();
    let mut stale_aliases = state.stale_alias_names_after_mutation(target.binding, &target.path);
    let supported = mutate_target(
        state,
        &target,
        origin,
        &EvaluatedMutation::Extend(extension),
    );
    state
        .mutations
        .insert(target.into_fact(PythonMutationOperation::Extend, origin));
    if supported {
        state.invalidate_names(
            stale_aliases,
            &PythonUnknownCause::UnsupportedExpression,
            origin,
        );
    } else {
        if !stale_aliases.contains(&binding) {
            stale_aliases.push(binding);
        }
        state.degrade_names(
            stale_aliases,
            &PythonUnknownCause::UnsupportedMutation,
            origin,
        );
    }
}

pub(super) fn walk_expr(evaluator: &mut Evaluator<'_>, expression: &ast::Expr) {
    let ast::Expr::Call(call) = expression else {
        evaluator.state.degrade_names(
            expr_read_names(expression),
            &PythonUnknownCause::UnsupportedExpression,
            evaluator.origin(expression),
        );
        return;
    };
    let ast::Expr::Attribute(attribute) = call.func.as_ref() else {
        evaluator.state.degrade_names(
            expr_read_names(expression),
            &PythonUnknownCause::UnsupportedMutation,
            evaluator.origin(expression),
        );
        return;
    };
    let Some(target) = MutationTarget::from_expr(&attribute.value) else {
        evaluator.state.degrade_names(
            expr_read_names(expression),
            &PythonUnknownCause::UnsupportedMutation,
            evaluator.origin(expression),
        );
        return;
    };
    let origin = evaluator.origin(call);
    let Some(operation) = PythonMutationOperation::from_method(attribute.attr.as_str()) else {
        let mut receiver_aliases = evaluator
            .state
            .stale_alias_names_after_mutation(target.binding, &target.path);
        if !receiver_aliases.iter().any(|name| name == target.binding) {
            receiver_aliases.push(target.binding.to_string());
        }
        evaluator.state.degrade_names(
            expr_read_names(expression),
            &PythonUnknownCause::UnsupportedMutation,
            origin,
        );
        evaluator.state.invalidate_names(
            receiver_aliases,
            &PythonUnknownCause::UnsupportedMutation,
            origin,
        );
        return;
    };
    let mut stale_aliases = evaluator
        .state
        .stale_alias_names_after_mutation(target.binding, &target.path);
    let supported = apply_mutation_operation(evaluator, call, &target, operation, origin);
    if supported {
        evaluator.state.invalidate_names(
            stale_aliases,
            &PythonUnknownCause::UnsupportedExpression,
            origin,
        );
        evaluator
            .state
            .mutations
            .insert(target.into_fact(operation, origin));
    } else {
        if !stale_aliases.iter().any(|name| name == target.binding) {
            stale_aliases.push(target.binding.to_string());
        }
        evaluator.state.degrade_names(
            expr_read_names(expression),
            &PythonUnknownCause::UnsupportedMutation,
            origin,
        );
        evaluator.state.invalidate_names(
            stale_aliases,
            &PythonUnknownCause::UnsupportedMutation,
            origin,
        );
    }
}

fn apply_mutation_operation(
    evaluator: &mut Evaluator<'_>,
    call: &ast::ExprCall,
    target: &MutationTarget<'_>,
    operation: PythonMutationOperation,
    origin: Origin,
) -> bool {
    if !call.arguments.keywords.is_empty()
        || call
            .arguments
            .args
            .iter()
            .any(|argument| matches!(argument, ast::Expr::Starred(_)))
    {
        return false;
    }
    let arguments = call
        .arguments
        .args
        .iter()
        .map(|argument| evaluator.evaluate_value(argument))
        .collect::<Vec<_>>();
    let mutation = match (operation, arguments.as_slice()) {
        (PythonMutationOperation::Append, [argument]) => EvaluatedMutation::Append(argument),
        (PythonMutationOperation::Extend, [extension]) => EvaluatedMutation::Extend(extension),
        (PythonMutationOperation::Insert, [_, argument]) => {
            let index = &call.arguments.args[0];
            let index = if let Some(index) = index.non_negative_integer() {
                InsertIndex::NonNegative(index)
            } else if let Some(index) = index.negative_integer() {
                InsertIndex::Negative(index)
            } else {
                return false;
            };
            EvaluatedMutation::Insert {
                index,
                value: argument,
            }
        }
        (PythonMutationOperation::Remove, [argument]) => {
            let PythonValueKind::Str(needle) = &argument.kind else {
                return false;
            };
            EvaluatedMutation::Remove(needle)
        }
        (
            PythonMutationOperation::Append
            | PythonMutationOperation::Extend
            | PythonMutationOperation::Insert
            | PythonMutationOperation::Remove,
            _,
        ) => return false,
    };
    mutate_target(&mut evaluator.state, target, origin, &mutation)
}

pub(super) fn extend_list_value(
    value: &mut PythonValue,
    extension: &PythonValue,
    origin: Origin,
) -> bool {
    if !value.is_mutable_container() {
        return false;
    }
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
    list.semantic_items()
        .iter()
        .all(|item| matches!(item, PythonListItem::Value(_)))
}

fn mutate_target(
    state: &mut EvaluationState,
    target: &MutationTarget<'_>,
    origin: Origin,
    mutation: &EvaluatedMutation<'_>,
) -> bool {
    let Some(binding) = state.bindings.get_mut(target.binding) else {
        return false;
    };
    let Some(bound) = binding.single_bound_mut() else {
        return false;
    };
    mutate_at_path(&mut bound.value, &target.path, origin, mutation)
}

fn mutate_at_path(
    value: &mut PythonValue,
    path: &[PythonMutationPathSegment],
    origin: Origin,
    mutation: &EvaluatedMutation<'_>,
) -> bool {
    let Some((next_segment, remaining)) = path.split_first() else {
        if !mutation.apply(value, origin) {
            return false;
        }
        value.record_origin(origin);
        return true;
    };

    let supported = match next_segment {
        PythonMutationPathSegment::Index(index) => {
            let PythonValueKind::List(list) = &mut value.kind else {
                return false;
            };
            list.try_mutate_indexed_value(*index, |next| {
                mutate_at_path(next, remaining, origin, mutation)
            })
        }
        PythonMutationPathSegment::Key(key) => {
            let PythonValueKind::Dict(dict) = &mut value.kind else {
                return false;
            };
            let mut selected = None;
            for item in dict.items.iter_mut().rev() {
                match item {
                    PythonDictItem::Entry {
                        key: candidate,
                        value,
                    } => match &candidate.kind {
                        PythonValueKind::Str(candidate) if candidate == key => {
                            selected = Some(value);
                            break;
                        }
                        PythonValueKind::Str(_) => {}
                        PythonValueKind::Unknown(_)
                        | PythonValueKind::Bool(_)
                        | PythonValueKind::Path(_)
                        | PythonValueKind::List(_)
                        | PythonValueKind::Dict(_) => return false,
                    },
                    PythonDictItem::UnknownUnpack(_) => return false,
                }
            }
            let Some(next) = selected else {
                return false;
            };
            mutate_at_path(next, remaining, origin, mutation)
        }
    };
    if supported {
        value.record_origin(origin);
    }
    supported
}

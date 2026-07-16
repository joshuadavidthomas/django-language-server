use djls_source::Origin;
use ruff_python_ast as ast;

use super::MutableOrigins;
use super::PythonDictItem;
use super::PythonListItem;
use super::PythonUnknownCause;
use super::PythonValue;
use super::PythonValueKind;
use super::evaluator::EvaluationState;
use super::evaluator::Evaluator;
use super::name_analysis::expr_read_names;
use crate::ast::ExprExt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonMutation {
    pub(crate) binding: String,
    pub(crate) path: PythonMutationPath,
    pub(crate) operation: PythonMutationOperation,
    pub(crate) origin: Origin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PythonMutationPathSegment {
    Index(usize),
    Key(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PythonMutationPath {
    segments: Vec<PythonMutationPathSegment>,
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

impl PythonMutationPath {
    fn from_expr(expr: &ast::Expr) -> Option<(&str, Self)> {
        if let Some(binding) = expr.name_target() {
            return Some((binding, Self::default()));
        }

        let ast::Expr::Subscript(subscript) = expr else {
            return None;
        };
        let segment = if let Some(index) = subscript.slice.non_negative_integer() {
            PythonMutationPathSegment::Index(index)
        } else if let Some(key) = subscript.slice.string_literal() {
            PythonMutationPathSegment::Key(key.to_string())
        } else {
            return None;
        };
        let (binding, mut path) = Self::from_expr(&subscript.value)?;
        path.segments.push(segment);
        Some((binding, path))
    }

    pub(crate) fn iter(&self) -> impl ExactSizeIterator<Item = &PythonMutationPathSegment> {
        self.segments.iter()
    }

    pub(crate) fn as_slice(&self) -> &[PythonMutationPathSegment] {
        &self.segments
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    pub(super) fn possible_target_origins(&self, value: &PythonValue) -> MutableOrigins {
        fn collect(
            value: &PythonValue,
            path: &[PythonMutationPathSegment],
            origins: &mut MutableOrigins,
        ) {
            let Some((segment, remaining)) = path.split_first() else {
                if matches!(
                    &value.kind,
                    PythonValueKind::List(_) | PythonValueKind::Dict(_)
                ) {
                    origins.extend(value.mutable_origins());
                }
                return;
            };
            match segment {
                PythonMutationPathSegment::Index(index) => {
                    let PythonValueKind::List(list) = &value.kind else {
                        return;
                    };
                    if let Some(PythonListItem::Value(value)) = list.semantic_items().get(*index) {
                        collect(value, remaining, origins);
                    }
                }
                PythonMutationPathSegment::Key(key) => {
                    let PythonValueKind::Dict(dict) = &value.kind else {
                        return;
                    };
                    for item in dict.items.iter().rev() {
                        let PythonDictItem::Entry {
                            key: candidate,
                            value,
                        } = item
                        else {
                            continue;
                        };
                        match &candidate.kind {
                            PythonValueKind::Str(candidate) if candidate == key => {
                                collect(value, remaining, origins);
                                return;
                            }
                            PythonValueKind::Str(_) => {}
                            PythonValueKind::Unknown(_)
                            | PythonValueKind::Bool(_)
                            | PythonValueKind::Path(_)
                            | PythonValueKind::List(_)
                            | PythonValueKind::Dict(_) => {
                                collect(value, remaining, origins);
                            }
                        }
                    }
                }
            }
        }

        let mut origins = MutableOrigins::default();
        collect(value, self.as_slice(), &mut origins);
        origins
    }

    fn try_apply_exact(
        &self,
        value: &mut PythonValue,
        mutation: &EvaluatedMutation<'_>,
        origin: Origin,
    ) -> bool {
        fn apply(
            value: &mut PythonValue,
            path: &[PythonMutationPathSegment],
            mutation: &EvaluatedMutation<'_>,
            origin: Origin,
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
                        apply(next, remaining, mutation, origin)
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
                    apply(next, remaining, mutation, origin)
                }
            };
            if supported {
                value.record_origin(origin);
            }
            supported
        }

        apply(value, self.as_slice(), mutation, origin)
    }
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
                return value.try_extend_from(extension, origin);
            }
            Self::Insert { index, value: item } => {
                let PythonValueKind::List(list) = &mut value.kind else {
                    return false;
                };
                if !list.is_authoritative() {
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
                if !list.is_authoritative() {
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
    path: PythonMutationPath,
}

impl<'a> MutationTarget<'a> {
    pub(super) fn from_expr(expr: &'a ast::Expr) -> Option<Self> {
        let (binding, path) = PythonMutationPath::from_expr(expr)?;
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

impl EvaluationState {
    pub(super) fn apply_augmented_add(
        &mut self,
        target: MutationTarget<'_>,
        extension: &PythonValue,
        origin: Origin,
    ) {
        let binding = target.binding.to_string();
        let mut stale_aliases = self.stale_alias_names_after_mutation(target.binding, &target.path);
        let supported =
            self.try_apply_mutation(&target, &EvaluatedMutation::Extend(extension), origin);
        self.mutations
            .insert(target.into_fact(PythonMutationOperation::Extend, origin));
        if supported {
            self.invalidate_names(
                stale_aliases,
                &PythonUnknownCause::UnsupportedExpression,
                origin,
            );
        } else {
            if !stale_aliases.contains(&binding) {
                stale_aliases.push(binding);
            }
            self.degrade_names(
                stale_aliases,
                &PythonUnknownCause::UnsupportedMutation,
                origin,
            );
        }
    }

    fn try_apply_mutation(
        &mut self,
        target: &MutationTarget<'_>,
        mutation: &EvaluatedMutation<'_>,
        origin: Origin,
    ) -> bool {
        let Some(binding) = self.bindings.get_mut(target.binding) else {
            return false;
        };
        let Some(bound) = binding.single_bound_mut() else {
            return false;
        };
        target
            .path
            .try_apply_exact(&mut bound.value, mutation, origin)
    }
}

impl Evaluator<'_> {
    pub(super) fn evaluate_expression_statement(&mut self, expression: &ast::Expr) {
        let ast::Expr::Call(call) = expression else {
            self.state.degrade_names(
                expr_read_names(expression),
                &PythonUnknownCause::UnsupportedExpression,
                self.origin(expression),
            );
            return;
        };
        let ast::Expr::Attribute(attribute) = call.func.as_ref() else {
            self.state.degrade_names(
                expr_read_names(expression),
                &PythonUnknownCause::UnsupportedMutation,
                self.origin(expression),
            );
            return;
        };
        let Some(target) = MutationTarget::from_expr(&attribute.value) else {
            self.state.degrade_names(
                expr_read_names(expression),
                &PythonUnknownCause::UnsupportedMutation,
                self.origin(expression),
            );
            return;
        };
        let origin = self.origin(call);
        let Some(operation) = PythonMutationOperation::from_method(attribute.attr.as_str()) else {
            let mut receiver_aliases = self
                .state
                .stale_alias_names_after_mutation(target.binding, &target.path);
            if !receiver_aliases.iter().any(|name| name == target.binding) {
                receiver_aliases.push(target.binding.to_string());
            }
            self.state.degrade_names(
                expr_read_names(expression),
                &PythonUnknownCause::UnsupportedMutation,
                origin,
            );
            self.state.invalidate_names(
                receiver_aliases,
                &PythonUnknownCause::UnsupportedMutation,
                origin,
            );
            return;
        };
        let mut stale_aliases = self
            .state
            .stale_alias_names_after_mutation(target.binding, &target.path);
        let supported = self.try_apply_mutation_call(call, &target, operation, origin);
        if supported {
            self.state.invalidate_names(
                stale_aliases,
                &PythonUnknownCause::UnsupportedExpression,
                origin,
            );
            self.state
                .mutations
                .insert(target.into_fact(operation, origin));
        } else {
            if !stale_aliases.iter().any(|name| name == target.binding) {
                stale_aliases.push(target.binding.to_string());
            }
            self.state.degrade_names(
                expr_read_names(expression),
                &PythonUnknownCause::UnsupportedMutation,
                origin,
            );
            self.state.invalidate_names(
                stale_aliases,
                &PythonUnknownCause::UnsupportedMutation,
                origin,
            );
        }
    }

    fn try_apply_mutation_call(
        &mut self,
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
            .map(|argument| self.evaluate_value(argument))
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
        self.state.try_apply_mutation(target, &mutation, origin)
    }
}

#[cfg(test)]
mod tests {
    use super::MutationTarget;
    use super::PythonMutationPathSegment;
    use super::ast;

    fn target(source: &str) -> Option<(String, Vec<PythonMutationPathSegment>)> {
        let parsed =
            ruff_python_parser::parse_module(source).expect("test expression should parse");
        let module = parsed.into_syntax();
        let [ast::Stmt::Expr(statement)] = module.body.as_slice() else {
            panic!("test source should contain one expression statement");
        };
        MutationTarget::from_expr(&statement.value).map(|target| {
            (
                target.binding.to_string(),
                target.path.iter().cloned().collect(),
            )
        })
    }

    #[test]
    fn mutation_path_is_root_to_leaf_by_construction() {
        assert_eq!(target("ROOT"), Some(("ROOT".to_string(), Vec::new())));
        assert_eq!(
            target("ROOT[0]['apps']"),
            Some((
                "ROOT".to_string(),
                vec![
                    PythonMutationPathSegment::Index(0),
                    PythonMutationPathSegment::Key("apps".to_string()),
                ],
            )),
        );
    }

    #[test]
    fn mutation_path_rejects_dynamic_indexes_and_keys() {
        assert_eq!(target("ROOT[index]"), None);
        assert_eq!(target("ROOT[key]"), None);
    }
}

use std::cmp::Ordering;

use djls_source::Origin;
use ruff_python_ast as ast;

use super::PythonSequenceItem;
use super::PythonUnknownCause;
use super::PythonValue;
use super::PythonValueKind;
use super::ReachableAllocationSites;
use super::StructuralOrd;
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

impl StructuralOrd for PythonMutation {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        self.binding
            .cmp(&other.binding)
            .then_with(|| self.path.structural_cmp(&other.path))
            .then_with(|| self.operation.structural_cmp(&other.operation))
            .then_with(|| self.origin.structural_cmp(&other.origin))
    }
}

impl StructuralOrd for PythonMutationPathSegment {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Index(left), Self::Index(right)) => left.cmp(right),
            (Self::Key(left), Self::Key(right)) => left.cmp(right),
            (Self::Index(_), Self::Key(_)) => Ordering::Less,
            (Self::Key(_), Self::Index(_)) => Ordering::Greater,
        }
    }
}

impl StructuralOrd for PythonMutationOperation {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        self.structural_rank().cmp(&other.structural_rank())
    }
}

impl PythonMutationOperation {
    /// Mutation facts retain their established operation-name precedence.
    fn structural_rank(self) -> u8 {
        match self {
            Self::Append => 0,
            Self::Extend => 1,
            Self::Insert => 2,
            Self::Remove => 3,
        }
    }

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

impl StructuralOrd for PythonMutationPath {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        for (left, right) in self.segments.iter().zip(&other.segments) {
            let ordering = left.structural_cmp(right);
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        self.segments.len().cmp(&other.segments.len())
    }
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

    pub(super) fn possible_target_allocation_sites(
        &self,
        value: &PythonValue,
    ) -> ReachableAllocationSites {
        fn collect(
            value: &PythonValue,
            path: &[PythonMutationPathSegment],
            origins: &mut ReachableAllocationSites,
        ) {
            let Some((segment, remaining)) = path.split_first() else {
                if let Some(sites) = value.own_mutable_sites() {
                    origins.push_group(sites.clone());
                }
                return;
            };
            match segment {
                PythonMutationPathSegment::Index(index) => {
                    // Index traversal reaches a nested value through either a
                    // list or a tuple; only the terminal mutation requires a
                    // mutable list.
                    let items = match &value.kind {
                        PythonValueKind::List(list) => list.semantic_items(),
                        PythonValueKind::Tuple(tuple) => tuple.semantic_items(),
                        PythonValueKind::Str(_)
                        | PythonValueKind::Bool(_)
                        | PythonValueKind::Path(_)
                        | PythonValueKind::Dict(_)
                        | PythonValueKind::Unknown(_) => return,
                    };
                    if let Some(PythonSequenceItem::Value(value)) = items.get(*index) {
                        collect(value, remaining, origins);
                    }
                }
                PythonMutationPathSegment::Key(key) => {
                    let PythonValueKind::Dict(dict) = &value.kind else {
                        return;
                    };
                    for next in dict.mapping().possible_string_values(key) {
                        collect(next, remaining, origins);
                    }
                }
            }
        }

        let mut origins = ReachableAllocationSites::default();
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
                    // Traverse into the nested value through a list or a tuple.
                    // The tuple's structure is never mutated here; only a nested
                    // mutable container reached through indexing can change.
                    match &mut value.kind {
                        PythonValueKind::List(list) => list
                            .try_mutate_indexed_value(*index, |next| {
                                apply(next, remaining, mutation, origin)
                            }),
                        PythonValueKind::Tuple(tuple) => tuple
                            .try_mutate_indexed_value(*index, |next| {
                                apply(next, remaining, mutation, origin)
                            }),
                        PythonValueKind::Str(_)
                        | PythonValueKind::Bool(_)
                        | PythonValueKind::Path(_)
                        | PythonValueKind::Dict(_)
                        | PythonValueKind::Unknown(_) => return false,
                    }
                }
                PythonMutationPathSegment::Key(key) => {
                    let PythonValueKind::Dict(dict) = &mut value.kind else {
                        return false;
                    };
                    dict.try_exact_string_value_mut(key, |next| {
                        apply(next, remaining, mutation, origin)
                    })
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
        let PythonValueKind::List(list) = &mut value.kind else {
            return false;
        };
        match self {
            Self::Append(argument) => list.append_value((*argument).clone()),
            Self::Extend(extension) => {
                return list.extend_from(extension, origin).is_some();
            }
            Self::Insert { index, value: item } => {
                if !list.is_authoritative() {
                    return false;
                }
                let len = list.len();
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
                list.insert_value(index, (*item).clone());
            }
            Self::Remove(needle) => {
                if !list.is_authoritative() {
                    return false;
                }
                return list.remove_str(needle);
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
        let fact = target.into_fact(PythonMutationOperation::Extend, origin);
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
        self.mutations.insert(fact);
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
    use std::cmp::Ordering;

    use djls_source::File;
    use djls_source::Origin;
    use djls_source::Span;
    use salsa::plumbing::FromId as _;

    use super::MutationTarget;
    use super::PythonMutation;
    use super::PythonMutationOperation;
    use super::PythonMutationPath;
    use super::PythonMutationPathSegment;
    use super::StructuralOrd;
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

    fn origin(file_index: u32, start: u32) -> Origin {
        // SAFETY: Synthetic files are compared only as opaque IDs and never read.
        let file = File::from_id(unsafe { salsa::Id::from_index(file_index) });
        Origin::new(file, Span::new(start, 1))
    }

    fn mutation(
        binding: &str,
        segments: Vec<PythonMutationPathSegment>,
        operation: PythonMutationOperation,
        origin: Origin,
    ) -> PythonMutation {
        PythonMutation {
            binding: binding.to_string(),
            path: PythonMutationPath { segments },
            operation,
            origin,
        }
    }

    #[test]
    fn typed_module_order_mutations_compare_every_field_and_reverse() {
        let base = mutation(
            "VALUE",
            vec![PythonMutationPathSegment::Index(1)],
            PythonMutationOperation::Append,
            origin(15, 1),
        );
        let unequal = [
            mutation(
                "VALUES",
                vec![PythonMutationPathSegment::Index(1)],
                PythonMutationOperation::Append,
                origin(15, 1),
            ),
            mutation(
                "VALUE",
                vec![PythonMutationPathSegment::Key("1".to_string())],
                PythonMutationOperation::Append,
                origin(15, 1),
            ),
            mutation(
                "VALUE",
                vec![PythonMutationPathSegment::Index(2)],
                PythonMutationOperation::Append,
                origin(15, 1),
            ),
            mutation(
                "VALUE",
                vec![PythonMutationPathSegment::Index(1)],
                PythonMutationOperation::Extend,
                origin(15, 1),
            ),
            mutation(
                "VALUE",
                vec![PythonMutationPathSegment::Index(1)],
                PythonMutationOperation::Append,
                origin(16, 1),
            ),
        ];

        assert_eq!(base.structural_cmp(&base), Ordering::Equal);
        for other in &unequal {
            assert_ne!(base.structural_cmp(other), Ordering::Equal);
            assert_eq!(
                base.structural_cmp(other),
                other.structural_cmp(&base).reverse()
            );
        }
    }

    #[test]
    fn typed_module_order_mutation_variants_have_exhaustive_precedence() {
        let segments = [
            PythonMutationPathSegment::Index(0),
            PythonMutationPathSegment::Key(String::new()),
        ];
        for (left_index, left) in segments.iter().enumerate() {
            for (right_index, right) in segments.iter().enumerate() {
                assert_eq!(left.structural_cmp(right), left_index.cmp(&right_index));
            }
        }

        let operations = [
            PythonMutationOperation::Append,
            PythonMutationOperation::Extend,
            PythonMutationOperation::Insert,
            PythonMutationOperation::Remove,
        ];
        for (left_index, left) in operations.iter().enumerate() {
            for (right_index, right) in operations.iter().enumerate() {
                assert_eq!(left.structural_cmp(right), left_index.cmp(&right_index));
            }
        }
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

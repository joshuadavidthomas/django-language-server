use std::cmp::Ordering;

use djls_source::Origin;
use ruff_python_ast as ast;

use super::PythonBinding;
use super::PythonBindingState;
use super::PythonSequenceItem;
use super::PythonUnknownCause;
use super::PythonValue;
use super::PythonValueKind;
use super::ReachableAllocationSites;
use super::StructuralOrd;
use super::evaluator::PythonEvaluationState;
use super::evaluator::PythonModuleEvaluator;
use super::name_analysis::reachable_expr_read_names;
use super::truthiness::Truthiness;
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
    SetKey {
        key: &'a str,
        key_origin: Origin,
        value: &'a PythonValue,
    },
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
                        | PythonValueKind::UnsupportedLiteral
                        | PythonValueKind::Dict(_)
                        | PythonValueKind::Module(_)
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
                        | PythonValueKind::UnsupportedLiteral
                        | PythonValueKind::Dict(_)
                        | PythonValueKind::Module(_)
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
    fn embedded_sites(&self) -> Option<ReachableAllocationSites> {
        match self {
            Self::Append(value) | Self::Insert { value, .. } | Self::SetKey { value, .. } => {
                Some(value.reachable_allocation_sites())
            }
            Self::Extend(value) => Some(value.iterated_reachable_allocation_sites()),
            Self::Remove(_) => None,
        }
    }

    fn apply(&self, value: &mut PythonValue, origin: Origin) -> bool {
        match self {
            Self::SetKey {
                key,
                key_origin,
                value: assigned,
            } => {
                let PythonValueKind::Dict(dictionary) = &mut value.kind else {
                    return false;
                };
                dictionary.append_entry(
                    PythonValue::string((*key).to_string(), *key_origin),
                    (*assigned).clone(),
                );
                true
            }
            Self::Append(argument) => {
                let PythonValueKind::List(list) = &mut value.kind else {
                    return false;
                };
                list.append_value((*argument).clone());
                true
            }
            Self::Extend(extension) => {
                let PythonValueKind::List(list) = &mut value.kind else {
                    return false;
                };
                list.extend_from(extension, origin).is_some()
            }
            Self::Insert { index, value: item } => {
                let PythonValueKind::List(list) = &mut value.kind else {
                    return false;
                };
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
                true
            }
            Self::Remove(needle) => {
                let PythonValueKind::List(list) = &mut value.kind else {
                    return false;
                };
                list.is_authoritative() && list.remove_str(needle)
            }
        }
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

impl PythonEvaluationState {
    pub(super) fn assign_string_key(
        &mut self,
        target: &MutationTarget<'_>,
        key: &str,
        key_origin: Origin,
        value: &PythonBinding,
        origin: Origin,
    ) {
        let mut stale_aliases = self.stale_alias_names_after_mutation(target.binding, &target.path);
        let applied = value.single_bound().is_some_and(|assigned| {
            self.try_apply_mutation(
                target,
                &EvaluatedMutation::SetKey {
                    key,
                    key_origin,
                    value: &assigned.value,
                },
                origin,
            )
        });
        if applied {
            self.invalidate_names(
                stale_aliases,
                &PythonUnknownCause::UnsupportedExpression,
                origin,
            );
        } else {
            if !stale_aliases.iter().any(|name| name == target.binding) {
                stale_aliases.push(target.binding.to_string());
            }
            self.degrade_names(
                stale_aliases,
                &PythonUnknownCause::UnsupportedMutation,
                origin,
            );
        }
    }

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

    pub(super) fn sites_reach_mutation_target(
        &self,
        target: &MutationTarget<'_>,
        sites: &ReachableAllocationSites,
    ) -> bool {
        let receiver_sites = self
            .bindings
            .get(target.binding)
            .map(|binding| {
                let mut sites = ReachableAllocationSites::default();
                for alternative in binding.alternatives() {
                    let PythonBindingState::Bound(bound) = alternative else {
                        continue;
                    };
                    sites.absorb(target.path.possible_target_allocation_sites(&bound.value));
                }
                sites
            })
            .unwrap_or_default();
        receiver_sites.intersects(sites)
    }

    fn try_apply_mutation(
        &mut self,
        target: &MutationTarget<'_>,
        mutation: &EvaluatedMutation<'_>,
        origin: Origin,
    ) -> bool {
        if mutation
            .embedded_sites()
            .is_some_and(|sites| self.sites_reach_mutation_target(target, &sites))
        {
            return false;
        }
        let Some(binding) = self.bindings.get_mut(target.binding) else {
            return false;
        };
        binding.try_mutate_all_bound(|value| target.path.try_apply_exact(value, mutation, origin))
    }
}

impl PythonModuleEvaluator<'_> {
    pub(super) fn evaluate_expression_statement(&mut self, expression: &ast::Expr) {
        let ast::Expr::Call(call) = expression else {
            self.record_unsupported_call_effects(expression);
            self.state.degrade_names(
                self.reachable_read_names(expression),
                &PythonUnknownCause::UnsupportedExpression,
                self.origin(expression),
            );
            return;
        };
        let ast::Expr::Attribute(attribute) = call.func.as_ref() else {
            self.state.degrade_unsupported_mutation_names(
                self.reachable_read_names(expression),
                self.origin(expression),
            );
            return;
        };
        let Some(target) = MutationTarget::from_expr(&attribute.value) else {
            self.state.degrade_unsupported_mutation_names(
                self.reachable_read_names(expression),
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
            self.state
                .degrade_unsupported_mutation_names(self.reachable_read_names(expression), origin);
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
            self.state
                .degrade_unsupported_mutation_names(self.reachable_read_names(expression), origin);
            self.state.invalidate_names(
                stale_aliases,
                &PythonUnknownCause::UnsupportedMutation,
                origin,
            );
        }
    }

    fn reachable_read_names(&self, expression: &ast::Expr) -> rustc_hash::FxHashSet<String> {
        reachable_expr_read_names(expression, &|value| {
            Truthiness::of_expr(value, &|name| self.state.known_truthiness(name))
        })
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
    use ruff_python_parser::parse_module;
    use salsa::Id;
    use salsa::plumbing::FromId as _;

    use super::MutationTarget;
    use super::PythonMutation;
    use super::PythonMutationOperation;
    use super::PythonMutationPath;
    use super::PythonMutationPathSegment;
    use super::StructuralOrd;
    use super::ast;

    fn target(source: &str) -> Option<(String, Vec<PythonMutationPathSegment>)> {
        let parsed = parse_module(source).expect("test expression should parse");
        let module = parsed.into_syntax();
        let statement = match module.body.as_slice() {
            [ast::Stmt::Expr(statement)] => Some(statement),
            _ => None,
        }?;
        MutationTarget::from_expr(&statement.value).map(|target| {
            (
                target.binding.to_string(),
                target.path.iter().cloned().collect(),
            )
        })
    }

    fn origin(file_index: u32, start: u32) -> Origin {
        // SAFETY: Synthetic files are compared only as opaque IDs and never read.
        let file = File::from_id(unsafe { Id::from_index(file_index) });
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

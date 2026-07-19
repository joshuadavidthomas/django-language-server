pub(super) mod expression;
mod imports;
mod statement;

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use djls_source::File;
use djls_source::Origin;
use djls_source::Span;
use ruff_python_ast as ast;

use super::BranchConstraints;
use super::PythonBinding;
use super::PythonBindingState;
use super::PythonImportOutcome;
use super::PythonModuleDependencies;
use super::PythonModuleValues;
use super::PythonMutation;
use super::PythonMutationOperation;
use super::PythonMutationPath;
use super::PythonMutationPathSegment;
use super::PythonNamespaceCause;
use super::PythonNamespaceRemainder;
use super::PythonUnknown;
use super::PythonUnknownCause;
use super::PythonValue;
use super::PythonValueKind;
use super::ReachableAllocationSites;
use super::UniqueVec;
use crate::ast::ExprExt;
use crate::ast::RangedExt;
use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonModule;
use crate::python::PythonPathBindings;

pub(super) fn evaluate_body(
    db: &dyn ProjectDb,
    project: Project,
    module: PythonModule,
    body: &[ast::Stmt],
) -> (PythonModuleValues, PythonModuleDependencies) {
    let state = EvaluationState::new(module.file());
    let mut evaluator = Evaluator {
        db,
        project,
        module,
        state,
    };
    evaluator.evaluate_body(body);
    evaluator.state.finish()
}

/// Context-bearing interpreter that evaluates Python syntax into a forkable state.
pub(super) struct Evaluator<'db> {
    db: &'db dyn ProjectDb,
    project: Project,
    module: PythonModule,
    pub(super) state: EvaluationState,
}

impl Evaluator<'_> {
    fn fork(&self) -> Self {
        Self {
            db: self.db,
            project: self.project,
            module: self.module.clone(),
            state: self.state.clone(),
        }
    }

    fn join_forks(&mut self, forks: Vec<Self>, origin: Origin) {
        let branches = forks
            .into_iter()
            .map(|evaluator| evaluator.state)
            .collect::<Vec<_>>();
        self.state = EvaluationState::join_branches(self.state.clone(), &branches, origin);
    }

    pub(super) fn origin<T: RangedExt>(&self, ranged: &T) -> Origin {
        Origin::new(self.module.file(), ranged.span())
    }

    fn origin_at(&self, span: Span) -> Origin {
        Origin::new(self.module.file(), span)
    }
}

/// Cloneable abstract environment for context-free evaluation transitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EvaluationState {
    pub(super) bindings: BTreeMap<String, PythonBinding>,
    namespace_causes: Vec<PythonNamespaceCause>,
    pub(super) mutations: UniqueVec<PythonMutation>,
    dependencies: PythonModuleDependencies,
}

impl EvaluationState {
    fn new(file: File) -> Self {
        Self {
            bindings: BTreeMap::new(),
            namespace_causes: Vec::new(),
            mutations: UniqueVec::new(),
            dependencies: PythonModuleDependencies::rooted(file),
        }
    }

    fn finish(self) -> (PythonModuleValues, PythonModuleDependencies) {
        (
            PythonModuleValues {
                bindings: self.bindings,
                namespace_remainder: (!self.namespace_causes.is_empty())
                    .then(|| PythonNamespaceRemainder::new(self.namespace_causes)),
                syntax_errors: Vec::new(),
                syntax_impacts: Vec::new(),
                mutations: self.mutations,
            },
            self.dependencies,
        )
    }

    fn binding(&self, name: &str) -> Option<&PythonBinding> {
        self.bindings.get(name)
    }

    fn assign_value(&mut self, name: &str, value: PythonValue, origin: Origin) {
        self.assign_binding(name, PythonBinding::bound(value, origin), origin);
    }

    /// Update a name's single bound value after a successful in-place mutation.
    /// This preserves the binding's assignment origins, branch constraints, and
    /// prior mutation facts; only rebinding operations replace that state.
    fn update_bound_value(&mut self, name: &str, value: PythonValue) {
        let bound = self
            .bindings
            .get_mut(name)
            .and_then(PythonBinding::single_bound_mut)
            .expect("name-target in-place mutation requires one bound value");
        bound.value = value;
    }

    fn assign_binding(&mut self, name: &str, binding: PythonBinding, origin: Origin) {
        self.mutations.retain(|mutation| mutation.binding != name);
        self.bindings
            .insert(name.to_string(), binding.rebase_binding_origin(origin));
    }

    fn assign_from_name(&mut self, name: &str, source: &str, origin: Origin) -> bool {
        let Some(binding) = self.binding(source).cloned() else {
            return false;
        };
        self.bindings
            .insert(name.to_string(), binding.rebase_binding_origin(origin));
        let copied = self
            .mutations
            .iter()
            .filter(|mutation| mutation.binding == source)
            .cloned()
            .map(|mut mutation| {
                mutation.binding = name.to_string();
                mutation
            })
            .collect::<Vec<_>>();
        self.mutations.retain(|mutation| mutation.binding != name);
        self.mutations.extend(copied);
        true
    }

    fn bind_unknown(&mut self, name: &str, cause: &PythonUnknownCause, origin: Origin) {
        self.assign_binding(name, PythonBinding::unknown(cause, origin), origin);
    }

    fn mutable_alias_names(&self, binding: &PythonBinding) -> Vec<String> {
        let wanted = binding.reachable_allocation_sites();
        if wanted.is_empty() {
            return Vec::new();
        }
        self.bindings
            .iter()
            .filter(|(_name, candidate)| candidate.reachable_allocation_sites().intersects(&wanted))
            .map(|(name, _binding)| name.clone())
            .collect()
    }

    pub(super) fn stale_alias_names_after_mutation(
        &self,
        name: &str,
        path: &PythonMutationPath,
    ) -> Vec<String> {
        let mut wanted = ReachableAllocationSites::default();
        let Some(binding) = self.binding(name) else {
            return Vec::new();
        };
        for state in binding.alternatives() {
            let PythonBindingState::Bound(bound) = state else {
                continue;
            };
            wanted.absorb(path.possible_target_allocation_sites(&bound.value));
        }
        if wanted.is_empty() {
            return Vec::new();
        }
        self.bindings
            .iter()
            .filter(|(candidate_name, candidate)| {
                let occurrences = candidate.allocation_site_occurrences(&wanted);
                occurrences > usize::from(candidate_name.as_str() == name)
            })
            .map(|(name, _binding)| name.clone())
            .collect()
    }

    fn value_for_name(&self, name: &str) -> Option<PythonValue> {
        let binding = self.binding(name)?;
        Some(binding.single_bound()?.value.clone())
    }

    fn bool_value(&self, name: &str) -> Option<bool> {
        let binding = self.binding(name)?;
        let mut values = binding.alternatives();
        let PythonBindingState::Bound(first) = values.next()? else {
            return None;
        };
        let PythonValueKind::Bool(value) = first.value.kind else {
            return None;
        };
        values
            .all(|alternative| {
                matches!(alternative, PythonBindingState::Bound(bound)
                    if matches!(bound.value.kind, PythonValueKind::Bool(other) if other == value))
            })
            .then_some(value)
    }

    fn path_bindings(&self) -> PythonPathBindings {
        let mut paths = PythonPathBindings::default();
        for (name, binding) in &self.bindings {
            let Some(bound) = binding.single_bound() else {
                continue;
            };
            if let PythonValueKind::Path(path) = &bound.value.kind {
                paths.set(name.clone(), path.clone());
            }
        }
        paths
    }

    fn degrade_all_bindings(
        &mut self,
        cause: &PythonUnknownCause,
        origin: Origin,
        constraints: &BranchConstraints,
    ) {
        for binding in self.bindings.values_mut() {
            let unknown = PythonBinding::constrained_unknown(cause, origin, constraints)
                .expect("a namespace cause must have a feasible branch");
            *binding = binding.clone().join(unknown, origin);
        }
    }

    pub(super) fn invalidate_names(
        &mut self,
        names: impl IntoIterator<Item = String>,
        cause: &PythonUnknownCause,
        origin: Origin,
    ) {
        for name in names {
            self.bind_unknown(&name, cause, origin);
        }
    }

    pub(super) fn degrade_names(
        &mut self,
        names: impl IntoIterator<Item = String>,
        cause: &PythonUnknownCause,
        origin: Origin,
    ) {
        let mut names = names.into_iter().collect::<BTreeSet<_>>();
        for name in names.clone() {
            if let Some(binding) = self.binding(&name) {
                names.extend(self.mutable_alias_names(binding));
            }
        }
        for name in names {
            let unknown = PythonBinding::unknown(cause, origin);
            let binding = match self.bindings.remove(&name) {
                Some(binding) => binding.join(unknown, origin),
                None => unknown,
            };
            self.bindings.insert(name, binding);
        }
    }

    fn degrade_loop_effects(mut self, evaluated_bodies: Vec<Self>, origin: Origin) -> Self {
        let changed_names = evaluated_bodies
            .iter()
            .flat_map(|body| body.changed_names_from(&self))
            .collect::<BTreeSet<_>>();

        for body in evaluated_bodies {
            let Self {
                bindings: _,
                namespace_causes,
                mutations,
                dependencies,
            } = body;
            self.namespace_causes.extend(namespace_causes);
            self.mutations.extend(mutations);
            self.dependencies.files.extend(dependencies.files);
            self.dependencies.imports.extend(dependencies.imports);
        }

        self.degrade_names(
            changed_names,
            &PythonUnknownCause::UnsupportedExpression,
            origin,
        );
        self
    }

    fn join_branches(mut base: Self, branches: &[Self], origin: Origin) -> Self {
        let names = branches
            .iter()
            .flat_map(|branch| branch.changed_names_from(&base))
            .collect::<BTreeSet<_>>();
        for name in names {
            let mut joined: Option<PythonBinding> = None;
            for (arm, branch) in branches.iter().enumerate() {
                let mut candidate = branch
                    .binding(&name)
                    .cloned()
                    .unwrap_or_else(PythonBinding::unbound);
                candidate.select_branch(origin, arm);
                joined = Some(match joined {
                    Some(current) => current.join(candidate, origin),
                    None => candidate,
                });
            }
            if let Some(binding) = joined {
                base.bindings.insert(name, binding);
            }
        }
        base.namespace_causes.clear();
        base.mutations.clear();
        base.dependencies = PythonModuleDependencies::default();
        for (arm, branch) in branches.iter().enumerate() {
            base.namespace_causes
                .extend(branch.namespace_causes.iter().cloned().map(|mut cause| {
                    cause.select_branch(origin, arm);
                    cause
                }));
            base.mutations.extend(branch.mutations.iter().cloned());
            base.dependencies
                .files
                .extend(branch.dependencies.files.iter().copied());
            base.dependencies
                .imports
                .extend(branch.dependencies.imports.iter().cloned());
        }
        base
    }

    fn changed_names_from(&self, base: &Self) -> BTreeSet<String> {
        let mut changed = base
            .bindings
            .keys()
            .chain(self.bindings.keys())
            .filter(|name| base.binding(name) != self.binding(name))
            .cloned()
            .collect::<BTreeSet<_>>();
        let mutation_roots = base
            .mutations
            .iter()
            .chain(&self.mutations)
            .map(|mutation| mutation.binding.as_str())
            .collect::<BTreeSet<_>>();
        for name in mutation_roots {
            if !base
                .rooted_mutation_evidence(name)
                .eq(self.rooted_mutation_evidence(name))
            {
                changed.insert(name.to_string());
            }
        }
        changed
    }

    fn rooted_mutation_evidence<'a>(
        &'a self,
        name: &'a str,
    ) -> impl Iterator<
        Item = (
            &'a [PythonMutationPathSegment],
            PythonMutationOperation,
            Origin,
        ),
    > + 'a {
        self.mutations
            .iter()
            .filter(move |mutation| mutation.binding == name)
            .map(|mutation| {
                (
                    mutation.path.as_slice(),
                    mutation.operation,
                    mutation.origin,
                )
            })
    }

    fn record_import(&mut self, outcome: PythonImportOutcome) {
        self.dependencies.imports.insert(outcome);
    }

    fn absorb_dependencies(&mut self, dependencies: &PythonModuleDependencies) {
        self.dependencies
            .files
            .extend(dependencies.files.iter().copied());
        self.dependencies
            .imports
            .extend(dependencies.imports.iter().cloned());
    }
}

#[cfg(test)]
mod tests {
    use djls_source::File;
    use djls_source::Span;
    use salsa::plumbing::FromId;
    use salsa::plumbing::Id;

    use super::EvaluationState;
    use super::Origin;
    use super::PythonBinding;
    use super::PythonImportOutcome;
    use super::PythonMutation;
    use super::PythonMutationOperation;
    use super::PythonMutationPath;
    use super::PythonNamespaceCause;
    use super::PythonUnknown;
    use super::PythonUnknownCause;
    use super::PythonValue;
    use crate::python::PythonModuleName;

    fn test_file(index: u64) -> File {
        File::from_id(Id::from_bits(index + 1))
    }

    fn origin(start: usize) -> Origin {
        Origin::new(test_file(0), Span::saturating_from_parts_usize(start, 1))
    }

    fn state_with_binding() -> EvaluationState {
        let mut state = EvaluationState::new(test_file(0));
        let binding_origin = origin(1);
        state.bindings.insert(
            "VALUE".to_string(),
            PythonBinding::bound(
                PythonValue::string("value".to_string(), binding_origin),
                binding_origin,
            ),
        );
        state
    }

    fn mutation(operation: PythonMutationOperation, start: usize) -> PythonMutation {
        PythonMutation {
            binding: "VALUE".to_string(),
            path: PythonMutationPath::default(),
            operation,
            origin: origin(start),
        }
    }

    #[test]
    fn changed_names_include_rooted_mutation_evidence() {
        let mut base = state_with_binding();
        base.mutations
            .insert(mutation(PythonMutationOperation::Append, 2));
        let mut changed = base.clone();
        changed.mutations = vec![mutation(PythonMutationOperation::Extend, 2)].into();

        assert_eq!(
            changed.changed_names_from(&base),
            ["VALUE".to_string()].into_iter().collect()
        );
    }

    #[test]
    fn changed_names_treat_rooted_mutation_order_as_semantic() {
        let mut base = state_with_binding();
        base.mutations = vec![
            mutation(PythonMutationOperation::Append, 2),
            mutation(PythonMutationOperation::Extend, 3),
        ]
        .into();
        let mut changed = base.clone();
        let mut reversed = changed.mutations.into_iter().collect::<Vec<_>>();
        reversed.reverse();
        changed.mutations = reversed.into();

        assert_eq!(
            changed.changed_names_from(&base),
            ["VALUE".to_string()].into_iter().collect()
        );
    }

    #[test]
    fn changed_names_ignore_fully_equal_states() {
        let mut base = state_with_binding();
        base.mutations
            .insert(mutation(PythonMutationOperation::Append, 2));

        assert!(base.clone().changed_names_from(&base).is_empty());
    }

    #[test]
    fn changed_names_include_constraint_only_binding_changes() {
        let base = state_with_binding();
        let mut changed = base.clone();
        changed
            .bindings
            .get_mut("VALUE")
            .expect("the fixture binding should exist")
            .select_branch(origin(2), 0);

        assert_eq!(
            changed.changed_names_from(&base),
            ["VALUE".to_string()].into_iter().collect()
        );
    }

    #[test]
    fn changed_names_ignore_namespace_dependency_and_import_only_changes() {
        let base = state_with_binding();
        let mut changed = base.clone();
        changed
            .namespace_causes
            .push(PythonNamespaceCause::unconstrained(PythonUnknown::new(
                PythonUnknownCause::UnsupportedExpression,
                [origin(2)],
            )));
        changed.dependencies.files.insert(test_file(1));
        changed
            .dependencies
            .imports
            .insert(PythonImportOutcome::NotFound {
                origin: origin(3),
                module: PythonModuleName::parse("missing").unwrap(),
            });

        assert!(changed.changed_names_from(&base).is_empty());
    }

    #[test]
    fn loop_effect_degradation_aggregates_effects_and_degrades_changed_names() {
        let base = state_with_binding();
        let loop_origin = origin(6);
        let expected_binding = base
            .binding("VALUE")
            .expect("the fixture binding should exist")
            .clone()
            .join(
                PythonBinding::unknown(&PythonUnknownCause::UnsupportedExpression, loop_origin),
                loop_origin,
            );
        let mut first_mutation = mutation(PythonMutationOperation::Extend, 2);
        first_mutation.binding = "OTHER".to_string();
        let mut later_mutation = mutation(PythonMutationOperation::Append, 3);
        later_mutation.binding = "OTHER".to_string();
        let first_cause = PythonNamespaceCause::unconstrained(PythonUnknown::new(
            PythonUnknownCause::UnsupportedExpression,
            [origin(4)],
        ));
        let second_cause = PythonNamespaceCause::unconstrained(PythonUnknown::new(
            PythonUnknownCause::UnsupportedMutation,
            [origin(5)],
        ));

        let mut first_body = base.clone();
        first_body.assign_value(
            "VALUE",
            PythonValue::string("changed".to_string(), origin(2)),
            origin(2),
        );
        first_body.mutations.insert(first_mutation.clone());
        first_body.dependencies.files.insert(test_file(1));
        first_body.namespace_causes.push(first_cause.clone());

        let mut second_body = base.clone();
        second_body.assign_value(
            "VALUE",
            PythonValue::string("changed".to_string(), origin(2)),
            origin(2),
        );
        second_body
            .mutations
            .extend([later_mutation.clone(), first_mutation.clone()]);
        second_body
            .dependencies
            .files
            .extend([test_file(2), test_file(1)]);
        second_body.namespace_causes.push(second_cause.clone());

        let degraded = base.degrade_loop_effects(vec![first_body, second_body], loop_origin);

        assert_eq!(degraded.binding("VALUE"), Some(&expected_binding));
        assert_eq!(
            degraded.dependencies.files.as_slice(),
            [test_file(0), test_file(1), test_file(2)]
        );
        assert_eq!(
            degraded.mutations.as_slice(),
            [first_mutation, later_mutation]
        );
        assert_eq!(degraded.namespace_causes, [first_cause, second_cause]);
    }

    #[test]
    fn branch_join_preserves_first_seen_mutation_order_and_deduplicates() {
        let base = EvaluationState::new(test_file(0));
        let first_seen = mutation(PythonMutationOperation::Extend, 2);
        let later = mutation(PythonMutationOperation::Append, 3);
        let mut first_branch = base.clone();
        first_branch.mutations.insert(first_seen.clone());
        let mut second_branch = base.clone();
        second_branch
            .mutations
            .extend([later.clone(), first_seen.clone()]);

        let joined =
            EvaluationState::join_branches(base, &[first_branch, second_branch], origin(4));

        assert_eq!(joined.mutations.as_slice(), [first_seen, later]);
    }
}

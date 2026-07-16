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
use super::PythonDict;
use super::PythonDictItem;
use super::PythonImportOutcome;
use super::PythonList;
use super::PythonListItem;
use super::PythonModuleDependencies;
use super::PythonModuleValues;
use super::PythonMutation;
use super::PythonMutationOperation;
use super::PythonMutationPathSegment;
use super::PythonNamespaceCause;
use super::PythonNamespaceRemainder;
use super::PythonUnknown;
use super::PythonUnknownCause;
use super::PythonValue;
use super::PythonValueKind;
use super::UniqueVec;
use crate::ast::ExprExt;
use crate::ast::RangedExt;
use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonModule;
use crate::python::PythonPathBindings;
use crate::python::evaluate_path;

pub(super) fn evaluate_body(
    db: &dyn ProjectDb,
    project: Project,
    module: PythonModule,
    body: &[ast::Stmt],
) -> (PythonModuleValues, PythonModuleDependencies) {
    let mut evaluator = Evaluator::new(db, project, module);
    evaluator.evaluate_body(body);
    evaluator.finish()
}

pub(super) struct Evaluator<'db> {
    db: &'db dyn ProjectDb,
    project: Project,
    module: PythonModule,
    pub(super) state: EvaluationState,
}

impl<'db> Evaluator<'db> {
    fn new(db: &'db dyn ProjectDb, project: Project, module: PythonModule) -> Self {
        let state = EvaluationState::new(module.file());
        Self {
            db,
            project,
            module,
            state,
        }
    }

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

    fn finish(self) -> (PythonModuleValues, PythonModuleDependencies) {
        self.state.finish()
    }

    pub(super) fn origin<T: RangedExt>(&self, ranged: &T) -> Origin {
        Origin::new(self.module.file(), ranged.span())
    }

    fn origin_at(&self, span: Span) -> Origin {
        Origin::new(self.module.file(), span)
    }
}

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
        let mut wanted = Vec::new();
        collect_mutable_value_origins_from_binding(binding, &mut wanted);
        self.bindings
            .iter()
            .filter(|(_name, candidate)| binding_contains_mutable_origin(candidate, &wanted))
            .map(|(name, _binding)| name.clone())
            .collect()
    }

    pub(super) fn stale_alias_names_after_mutation(
        &self,
        name: &str,
        path: &[PythonMutationPathSegment],
    ) -> Vec<String> {
        let mut wanted = Vec::new();
        let Some(binding) = self.binding(name) else {
            return Vec::new();
        };
        for state in binding.alternatives() {
            let PythonBindingState::Bound(bound) = state else {
                continue;
            };
            collect_mutation_target_origins(&bound.value, path, &mut wanted);
        }
        self.bindings
            .iter()
            .filter(|(candidate_name, candidate)| {
                let occurrences = binding_mutable_origin_count(candidate, &wanted);
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

    fn apply_star_import(&mut self, values: &PythonModuleValues, import_origin: Origin) {
        if let Some(remainder) = &values.namespace_remainder {
            for cause in &remainder.causes {
                self.degrade_all_bindings(&cause.unknown.cause, import_origin, &cause.constraints);
            }
        }
        for (name, binding) in &values.bindings {
            let prior = self.bindings.get(name).cloned();
            let mut binding = binding.clone();
            rebase_cycle_unknowns(&mut binding, import_origin);
            self.bindings.insert(
                name.clone(),
                binding.replace_unbound_with(prior, import_origin),
            );
        }
        let mut namespace_errors = Vec::new();
        for impact in &values.syntax_impacts {
            let affected = self
                .bindings
                .keys()
                .filter(|name| impact.affects(name))
                .cloned()
                .collect::<Vec<_>>();
            if !affected.is_empty() {
                self.degrade_names(
                    affected,
                    &PythonUnknownCause::SyntaxErrors(vec![impact.error.clone()]),
                    import_origin,
                );
            }
            if impact.namespace_open {
                namespace_errors.push(impact.error.clone());
            }
        }
        if !namespace_errors.is_empty() {
            self.namespace_causes
                .push(PythonNamespaceCause::unconstrained(PythonUnknown {
                    cause: PythonUnknownCause::SyntaxErrors(namespace_errors),
                    origin: Some(import_origin),
                }));
        }
        self.mutations.extend(values.mutations.iter().cloned());
        if let Some(remainder) = &values.namespace_remainder {
            self.namespace_causes
                .extend(remainder.causes.iter().cloned().map(|mut cause| {
                    cause.unknown.origin = Some(import_origin);
                    cause
                }));
        }
    }

    fn bind_named_import(
        &mut self,
        values: &PythonModuleValues,
        imported_name: &str,
        bound_name: &str,
        origin: Origin,
    ) {
        let syntax_errors = values
            .syntax_impacts
            .iter()
            .filter(|impact| impact.affects(imported_name))
            .map(|impact| impact.error.clone())
            .collect::<Vec<_>>();
        let mut binding = values
            .bindings
            .get(imported_name)
            .cloned()
            .unwrap_or_else(PythonBinding::unbound)
            .rebase_binding_origin(origin);
        rebase_cycle_unknowns(&mut binding, origin);

        let unbound_constraints = binding
            .alternatives_with_constraints()
            .filter_map(|(alternative, constraints)| {
                (*alternative == PythonBindingState::Unbound).then_some(constraints.clone())
            })
            .collect::<Vec<_>>();
        if let Some(remainder) = &values.namespace_remainder {
            for unbound in &unbound_constraints {
                for cause in &remainder.causes {
                    let constraints = unbound.intersection(&cause.constraints);
                    if let Some(unknown) = PythonBinding::constrained_unknown(
                        &cause.unknown.cause,
                        origin,
                        &constraints,
                    ) {
                        binding = binding.join(unknown, origin);
                    }
                }
            }
        }
        if !syntax_errors.is_empty() {
            let unknown =
                PythonBinding::unknown(&PythonUnknownCause::SyntaxErrors(syntax_errors), origin);
            binding = binding.join(unknown, origin);
        }
        self.bindings.insert(bound_name.to_string(), binding);
        let copied = values
            .mutations
            .iter()
            .filter(|mutation| mutation.binding == imported_name)
            .cloned()
            .map(|mut mutation| {
                mutation.binding = bound_name.to_string();
                mutation
            })
            .collect::<Vec<_>>();
        self.mutations.extend(copied);
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

fn rebase_cycle_unknowns(binding: &mut PythonBinding, origin: Origin) {
    for state in binding.alternatives_mut() {
        let PythonBindingState::Bound(bound) = state else {
            continue;
        };
        let PythonValueKind::Unknown(unknown) = &mut bound.value.kind else {
            continue;
        };
        if unknown.cause == PythonUnknownCause::Cycle {
            unknown.origin = Some(origin);
            bound.binding_origins = vec![origin];
            bound.value.rebase_origin(origin);
        }
    }
}

fn collect_mutation_target_origins(
    value: &PythonValue,
    path: &[PythonMutationPathSegment],
    origins: &mut Vec<Origin>,
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
                collect_mutation_target_origins(value, remaining, origins);
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
                        collect_mutation_target_origins(value, remaining, origins);
                        return;
                    }
                    PythonValueKind::Str(_) => {}
                    PythonValueKind::Unknown(_)
                    | PythonValueKind::Bool(_)
                    | PythonValueKind::Path(_)
                    | PythonValueKind::List(_)
                    | PythonValueKind::Dict(_) => {
                        collect_mutation_target_origins(value, remaining, origins);
                    }
                }
            }
        }
    }
}

fn collect_mutable_value_origins_from_binding(binding: &PythonBinding, origins: &mut Vec<Origin>) {
    for state in binding.alternatives() {
        if let PythonBindingState::Bound(bound) = state {
            collect_mutable_value_origins(&bound.value, origins);
        }
    }
}

fn collect_mutable_value_origins(value: &PythonValue, origins: &mut Vec<Origin>) {
    match &value.kind {
        PythonValueKind::List(list) => {
            origins.extend(value.mutable_origins());
            for item in list.semantic_items() {
                if let PythonListItem::Value(value) = item {
                    collect_mutable_value_origins(value, origins);
                }
            }
        }
        PythonValueKind::Dict(dict) => {
            origins.extend(value.mutable_origins());
            for item in &dict.items {
                if let PythonDictItem::Entry { key, value } = item {
                    collect_mutable_value_origins(key, origins);
                    collect_mutable_value_origins(value, origins);
                }
            }
        }
        PythonValueKind::Unknown(_)
        | PythonValueKind::Str(_)
        | PythonValueKind::Bool(_)
        | PythonValueKind::Path(_) => {}
    }
}

fn binding_contains_mutable_origin(binding: &PythonBinding, wanted: &[Origin]) -> bool {
    binding_mutable_origin_count(binding, wanted) > 0
}

fn binding_mutable_origin_count(binding: &PythonBinding, wanted: &[Origin]) -> usize {
    binding
        .alternatives()
        .filter_map(|state| match state {
            PythonBindingState::Bound(bound) => Some(mutable_origin_count(&bound.value, wanted)),
            PythonBindingState::Unbound => None,
        })
        .sum()
}

fn mutable_origin_count(value: &PythonValue, wanted: &[Origin]) -> usize {
    let own = usize::from(
        value
            .mutable_origins()
            .any(|origin| wanted.contains(&origin)),
    );
    own + match &value.kind {
        PythonValueKind::List(list) => list
            .semantic_items()
            .iter()
            .filter_map(|item| match item {
                PythonListItem::Value(value) => Some(mutable_origin_count(value, wanted)),
                PythonListItem::UnknownElement(_) | PythonListItem::UnknownUnpack(_) => None,
            })
            .sum::<usize>(),
        PythonValueKind::Dict(dict) => dict
            .items
            .iter()
            .filter_map(|item| match item {
                PythonDictItem::Entry { key, value } => {
                    Some(mutable_origin_count(key, wanted) + mutable_origin_count(value, wanted))
                }
                PythonDictItem::UnknownUnpack(_) => None,
            })
            .sum(),
        PythonValueKind::Unknown(_)
        | PythonValueKind::Str(_)
        | PythonValueKind::Bool(_)
        | PythonValueKind::Path(_) => 0,
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
    use super::PythonNamespaceCause;
    use super::PythonUnknown;
    use super::PythonUnknownCause;
    use super::PythonValue;
    use super::PythonValueKind;
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
                PythonValue::known(PythonValueKind::Str("value".to_string()), binding_origin),
                binding_origin,
            ),
        );
        state
    }

    fn mutation(operation: PythonMutationOperation, start: usize) -> PythonMutation {
        PythonMutation {
            binding: "VALUE".to_string(),
            path: Vec::new(),
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
            .push(PythonNamespaceCause::unconstrained(PythonUnknown {
                cause: PythonUnknownCause::UnsupportedExpression,
                origin: Some(origin(2)),
            }));
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

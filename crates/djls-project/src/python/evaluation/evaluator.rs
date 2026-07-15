pub(super) mod expression;
mod imports;
mod statement;

use std::collections::BTreeMap;

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
use super::PythonModuleEvaluation;
use super::PythonModuleValues;
use super::PythonModuleValuesOutcome;
use super::PythonMutation;
use super::PythonNamespaceCause;
use super::PythonNamespaceRemainder;
use super::PythonUnknown;
use super::PythonUnknownCause;
use super::PythonValue;
use super::PythonValueKind;
use super::extend_ordered_unique;
use super::touched_names::TouchedNames;
use crate::ast::ExprExt;
use crate::ast::RangedExt;
use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonImportRequest;
use crate::python::PythonModule;
use crate::python::PythonPathBindings;
use crate::python::evaluate_path;

pub(super) fn evaluate_body(
    db: &dyn ProjectDb,
    project: Project,
    file: File,
    body: &[ast::Stmt],
) -> PythonModuleEvaluation {
    let context = EvaluationContext { db, project, file };
    let state = EvaluationState::new(file);
    statement::evaluate_body(context, state, body).finish()
}

pub(super) struct EvaluationContext<'db> {
    db: &'db dyn ProjectDb,
    project: Project,
    file: File,
}

impl EvaluationContext<'_> {
    pub(super) fn origin<T: RangedExt>(&self, ranged: &T) -> Origin {
        Origin::new(self.file, ranged.span())
    }

    fn origin_at(&self, span: Span) -> Origin {
        Origin::new(self.file, span)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EvaluationState {
    pub(super) bindings: BTreeMap<String, PythonBinding>,
    namespace_causes: Vec<PythonNamespaceCause>,
    pub(super) mutations: Vec<PythonMutation>,
    dependencies: PythonModuleDependencies,
}

impl EvaluationState {
    fn new(file: File) -> Self {
        Self {
            bindings: BTreeMap::new(),
            namespace_causes: Vec::new(),
            mutations: Vec::new(),
            dependencies: PythonModuleDependencies::rooted(file),
        }
    }

    fn finish(self) -> PythonModuleEvaluation {
        PythonModuleEvaluation::evaluated(
            PythonModuleValuesOutcome::Readable(PythonModuleValues {
                bindings: self.bindings,
                namespace_remainder: (!self.namespace_causes.is_empty())
                    .then(|| PythonNamespaceRemainder::new(self.namespace_causes)),
                syntax_errors: Vec::new(),
                syntax_impacts: Vec::new(),
                mutations: self.mutations,
            }),
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
        extend_ordered_unique(&mut self.mutations, &values.mutations);
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
        extend_ordered_unique(&mut self.mutations, &copied);
    }

    fn join_branches(
        mut base: Self,
        branches: &[Self],
        writes: &TouchedNames,
        origin: Origin,
    ) -> Self {
        let mut names = writes.names.clone();
        for branch in branches {
            for (name, binding) in &branch.bindings {
                if base.binding(name) != Some(binding) {
                    names.insert(name.clone());
                }
            }
        }
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
            extend_ordered_unique(&mut base.mutations, &branch.mutations);
            extend_ordered_unique(&mut base.dependencies.files, &branch.dependencies.files);
            extend_ordered_unique(&mut base.dependencies.imports, &branch.dependencies.imports);
        }
        base
    }

    fn changed_writes_from(base: &Self, changed: &Self) -> TouchedNames {
        let mut writes = TouchedNames::default();
        for (name, binding) in &changed.bindings {
            if base.binding(name) != Some(binding) {
                writes.record(name);
            }
        }
        for name in base.bindings.keys() {
            if changed.binding(name).is_none() {
                writes.record(name);
            }
        }
        writes
    }

    fn record_import(&mut self, outcome: PythonImportOutcome) {
        if !self.dependencies.imports.contains(&outcome) {
            self.dependencies.imports.push(outcome);
        }
    }

    fn absorb_dependencies(&mut self, evaluation: &PythonModuleEvaluation) {
        extend_ordered_unique(&mut self.dependencies.files, &evaluation.dependencies.files);
        extend_ordered_unique(
            &mut self.dependencies.imports,
            &evaluation.dependencies.imports,
        );
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

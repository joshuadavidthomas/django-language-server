use djls_source::Origin;

use super::bindings::PythonBinding;
use super::bindings::PythonBindings;
use super::model::PythonSemanticModel;
use super::mutation_target::MutationAccess;
use super::mutation_target::MutationTarget;
use super::mutations::PythonMutation;
use super::mutations::PythonMutations;
use super::touched_names::TouchedNames;
use super::values::PythonValue;
use crate::python::PythonPathBindings;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct PythonSemanticState {
    bindings: PythonBindings,
    mutations: PythonMutations,
}

impl PythonSemanticState {
    pub(super) fn into_parts(self) -> (PythonBindings, PythonMutations) {
        (self.bindings, self.mutations)
    }

    pub(super) fn assign_from_name(
        &mut self,
        name: &str,
        source_name: &str,
        binding_origin: Origin,
    ) -> bool {
        let Some(binding) = self.bindings.get(source_name).cloned() else {
            return false;
        };

        let mut copied_binding = binding;
        copied_binding.name = name.to_string();
        for bound in &mut copied_binding.values {
            bound.binding_origin = binding_origin;
        }
        self.bindings.bind(name, copied_binding);
        self.mutations
            .replace_root_from_assignment(source_name, name);
        true
    }

    pub(super) fn assign_value(&mut self, name: &str, value: PythonValue, origin: Origin) {
        self.mutations.remove_root(name);
        self.bindings
            .bind(name, PythonBinding::full(name, value, origin));
    }

    pub(super) fn apply_star_import(&mut self, imported_model: &PythonSemanticModel) {
        self.bindings.merge_star_import(imported_model);
        self.mutations.extend_from(imported_model.mutation_set());
    }

    pub(super) fn bind_named_import(
        &mut self,
        imported_model: &PythonSemanticModel,
        imported_name: &str,
        bound_name: &str,
        origin: Origin,
    ) {
        self.mutations.extend_renamed_root_from(
            imported_model.mutation_set(),
            imported_name,
            bound_name,
        );

        if let Some(binding) = imported_model.binding(imported_name).cloned() {
            let mut binding = binding;
            binding.name = bound_name.to_string();
            self.bindings.bind(bound_name, binding);
        } else {
            self.bind_unknown(bound_name, origin);
        }
    }

    pub(super) fn bind_unknown(&mut self, name: &str, origin: Origin) {
        self.bindings
            .bind(name, PythonBinding::unknown(name, origin));
    }

    pub(super) fn mark_all_partial(&mut self) {
        self.bindings.mark_all_partial();
    }

    pub(super) fn bool_value(&self, name: &str) -> Option<bool> {
        self.bindings.bool_value(name)
    }

    pub(super) fn path_bindings(&self) -> PythonPathBindings {
        self.bindings.path_bindings()
    }

    pub(super) fn value_for_name(&self, name: &str) -> Option<PythonValue> {
        let binding = self.bindings.get(name)?;
        let [bound] = binding.values() else {
            return None;
        };
        let mut value = bound.value().clone();
        if !binding.is_complete() || !bound.is_complete() {
            value.mark_partial();
        }
        Some(value)
    }

    pub(super) fn record_mutation(&mut self, target: &MutationTarget<'_>, method: &str) {
        self.mutations.insert(PythonMutation {
            root: target.root.to_string(),
            access: target
                .access
                .iter()
                .map(MutationAccess::to_public)
                .collect(),
            method: method.to_string(),
        });
    }

    pub(super) fn mutate_target(
        &mut self,
        target: &MutationTarget<'_>,
        mutate: impl FnOnce(&mut PythonValue) -> bool,
    ) -> bool {
        let Some(binding) = self.bindings.by_name.get_mut(target.root) else {
            return false;
        };
        let [bound] = binding.values.as_mut_slice() else {
            binding.mark_partial();
            return true;
        };
        let Some(value) = target.resolve_mut(&mut bound.value) else {
            binding.mark_partial();
            return true;
        };
        let mutated = mutate(value);
        if mutated && !value.is_complete() {
            binding.mark_partial();
        }
        mutated
    }

    pub(super) fn degrade_names(
        &mut self,
        names: impl IntoIterator<Item = String>,
        origin: Origin,
    ) {
        for name in names {
            if let Some(binding) = self.bindings.by_name.get_mut(&name) {
                binding.mark_partial();
            } else {
                self.bindings
                    .bind(&name, PythonBinding::unknown(&name, origin));
            }
        }
    }

    pub(super) fn changed_writes_from(base: &Self, changed: &Self) -> TouchedNames {
        let mut writes = TouchedNames::default();
        for (name, binding) in &changed.bindings.by_name {
            if base.bindings.get(name) != Some(binding) {
                writes.record(name);
            }
        }
        for name in base.bindings.by_name.keys() {
            if !changed.bindings.by_name.contains_key(name) {
                writes.record(name);
            }
        }
        for mutation in changed.mutations.iter() {
            if !base.mutations.contains(mutation) {
                writes.record(mutation.root());
            }
        }
        writes
    }

    pub(super) fn join_branches(mut base: Self, branches: &[Self], writes: &TouchedNames) -> Self {
        let mut names = writes.names.clone();
        for branch in branches {
            for (name, binding) in &branch.bindings.by_name {
                if base.bindings.get(name) != Some(binding) {
                    names.insert(name.clone());
                }
            }
        }

        for name in &names {
            let branch_values = branches
                .iter()
                .map(|branch| branch.bindings.get(name).cloned());
            if let Some(binding) = PythonBinding::joined(name, branch_values) {
                base.bindings.bind(name, binding);
            } else {
                base.bindings.remove(name);
            }
        }

        base.mutations.clear();
        for branch in branches {
            base.mutations.extend_from(&branch.mutations);
        }

        base
    }
}

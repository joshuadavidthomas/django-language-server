use djls_source::Origin;

use super::model::PythonBinding;
use super::model::PythonBindings;
use super::model::PythonMutations;
use super::touched_names::TouchedNames;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct PythonSemanticState {
    pub(super) bindings: PythonBindings,
    pub(super) mutations: PythonMutations,
}

impl PythonSemanticState {
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

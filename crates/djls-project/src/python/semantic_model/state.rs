use camino::Utf8PathBuf;
use djls_source::File;
use djls_source::Origin;

use super::model::PythonBinding;
use super::model::PythonBindings;
use super::model::PythonMutation;
use super::model::PythonSemanticModel;
use super::touched_names::TouchedNames;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct PythonSemanticState {
    pub(super) bindings: PythonBindings,
    pub(super) mutations: Vec<PythonMutation>,
    pub(super) effects: PythonSemanticEffects,
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

    pub(super) fn merge_only_effects_from_state(&mut self, other: Self) {
        self.effects.merge(other.effects);
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
        for mutation in &changed.mutations {
            if !base.mutations.contains(mutation) {
                writes.record(&mutation.root);
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
        base.effects = PythonSemanticEffects::default();
        for branch in branches {
            for mutation in &branch.mutations {
                if !base.mutations.contains(mutation) {
                    base.mutations.push(mutation.clone());
                }
            }
            base.effects.merge(branch.effects.clone());
        }

        base
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct PythonSemanticEffects {
    pub(super) imported_models: Vec<PythonSemanticModel>,
    pub(super) read_failures: Vec<(File, Utf8PathBuf)>,
}

impl PythonSemanticEffects {
    pub(super) fn add_imported_model(&mut self, imported_model: PythonSemanticModel) {
        let Some(root_file) = imported_model.files_read.first().copied() else {
            return;
        };
        if !self
            .imported_models
            .iter()
            .any(|existing| existing.files_read.first().copied() == Some(root_file))
        {
            self.imported_models.push(imported_model);
        }
    }

    pub(super) fn add_read_failure(&mut self, file: File, path: Utf8PathBuf) {
        if !self
            .read_failures
            .iter()
            .any(|(existing_file, existing_path)| *existing_file == file && *existing_path == path)
        {
            self.read_failures.push((file, path));
        }
    }

    pub(super) fn merge(&mut self, other: Self) {
        for imported_model in other.imported_models {
            self.add_imported_model(imported_model);
        }
        for (file, path) in other.read_failures {
            self.add_read_failure(file, path);
        }
    }
}

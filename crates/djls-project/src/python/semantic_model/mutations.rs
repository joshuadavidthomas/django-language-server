#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct PythonMutations(Vec<PythonMutation>);

impl PythonMutations {
    pub(super) fn as_slice(&self) -> &[PythonMutation] {
        &self.0
    }

    pub(super) fn insert(&mut self, mutation: PythonMutation) {
        if !self.0.contains(&mutation) {
            self.0.push(mutation);
        }
    }

    pub(super) fn clear(&mut self) {
        self.0.clear();
    }

    pub(super) fn remove_root(&mut self, root: &str) {
        self.0.retain(|mutation| mutation.root != root);
    }

    pub(super) fn contains(&self, mutation: &PythonMutation) -> bool {
        self.0.contains(mutation)
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &PythonMutation> {
        self.0.iter()
    }

    pub(super) fn replace_root_from_assignment(&mut self, source_root: &str, target_root: &str) {
        let copied_mutations = self
            .0
            .iter()
            .filter(|mutation| mutation.root == source_root)
            .map(|mutation| mutation.renamed_root(target_root))
            .collect::<Vec<_>>();
        self.remove_root(target_root);
        for mutation in copied_mutations {
            self.insert(mutation);
        }
    }

    pub(super) fn extend_from(&mut self, mutations: &Self) {
        for mutation in mutations.iter() {
            self.insert(mutation.clone());
        }
    }

    pub(super) fn extend_renamed_root_from(
        &mut self,
        mutations: &Self,
        source_root: &str,
        target_root: &str,
    ) {
        for mutation in mutations.iter() {
            if mutation.root == source_root {
                self.insert(mutation.renamed_root(target_root));
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonMutation {
    pub(super) root: String,
    pub(super) access: Vec<PythonMutationAccess>,
    pub(super) method: String,
}

impl PythonMutation {
    fn renamed_root(&self, root: &str) -> Self {
        let mut mutation = self.clone();
        mutation.root = root.to_string();
        mutation
    }

    pub(crate) fn root(&self) -> &str {
        &self.root
    }

    pub(crate) fn access(&self) -> &[PythonMutationAccess] {
        &self.access
    }

    pub(crate) fn method(&self) -> &str {
        &self.method
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PythonMutationAccess {
    Index(usize),
    Key(String),
}

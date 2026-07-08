use super::model::PythonMutation;
use super::model::PythonSemanticModel;
use super::source_graph::PythonImportEdge;
use super::source_graph::PythonImportKind;

pub(super) fn mutations_for_import(
    edge: &PythonImportEdge,
    imported_model: &PythonSemanticModel,
) -> Vec<PythonMutation> {
    match edge.kind() {
        PythonImportKind::Star => imported_model.mutations().to_vec(),
        PythonImportKind::Named => {
            let mut mutations = Vec::new();
            for (imported_name, bound_name) in edge.named_imports() {
                for mutation in imported_model.mutations() {
                    if mutation.root == imported_name {
                        let mut mutation = mutation.clone();
                        mutation.root = bound_name.to_string();
                        mutations.push(mutation);
                    }
                }
            }
            mutations
        }
    }
}

pub(super) fn push_mutation_once(mutations: &mut Vec<PythonMutation>, mutation: PythonMutation) {
    if !mutations.contains(&mutation) {
        mutations.push(mutation);
    }
}

pub(super) fn push_mutations(target: &mut Vec<PythonMutation>, mutations: Vec<PythonMutation>) {
    for mutation in mutations {
        push_mutation_once(target, mutation);
    }
}

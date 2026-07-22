mod discovery;
mod extract;
mod graph;
mod imports;
mod resolve;

use std::collections::BTreeMap;

pub use discovery::model_modules;
use djls_source::File;
pub(crate) use graph::AncestryOutcome;
pub(crate) use graph::BaseOutcome;
pub(crate) use graph::BaseUnresolvedReason;
pub(crate) use graph::InheritanceError;
pub use graph::ModelGraph;
pub use graph::ModelId;
pub(crate) use resolve::resolve_local_model_graph;

use crate::db::Db;
use crate::models::extract::ModelExtraction;
use crate::models::extract::extract_models_impl;
use crate::models::resolve::resolve_model_inheritance;
use crate::project::Project;
use crate::python::PythonModuleName;
use crate::python::RecoveredPythonModule;
use crate::python::import::ModuleKind;

/// Compute a merged `ModelGraph` from discovered model sources.
#[salsa::tracked(returns(ref))]
pub fn compute_model_graph(db: &dyn Db, project: Project) -> ModelGraph {
    // Module selection is last-wins. Iterate search paths in reverse so the
    // earlier Python search path supplies the final file for each module.
    resolve_model_graph_from_modules(
        db,
        project,
        model_modules(db, project)
            .iter()
            .rev()
            .map(|module| (module.file(), module.name().clone())),
    )
}

pub fn resolve_model_graph_from_modules(
    db: &dyn Db,
    project: Project,
    modules: impl IntoIterator<Item = (File, PythonModuleName)>,
) -> ModelGraph {
    let mut selected_modules = BTreeMap::new();
    for (file, module_name) in modules {
        selected_modules.insert(module_name, file);
    }

    let mut candidates = Vec::new();
    for (module_name, file) in selected_modules {
        candidates.extend(
            extract_models(db, file, module_name)
                .candidates
                .iter()
                .cloned(),
        );
    }

    let mut graph = resolve_model_inheritance(db, project, candidates);
    graph.resolve_relation_targets(db, project);
    #[cfg(debug_assertions)]
    graph.debug_assert_no_file_local_placeholders();
    graph
}

/// Extract models from one Python file, cached by Salsa.
///
/// This is a separate query from `compute_model_graph` so project-wide graph
/// recomputation can reuse unchanged per-file data.
// Salsa tracked-query keys are by-value; `module_name` is a key, not a borrow.
#[allow(clippy::needless_pass_by_value)]
#[salsa::tracked(returns(ref))]
pub fn extract_models(
    db: &dyn djls_source::Db,
    file: File,
    module_name: PythonModuleName,
) -> ModelExtraction {
    let Ok(Some(module)) = RecoveredPythonModule::from_file(db, file) else {
        return ModelExtraction::default();
    };

    let module_kind = if file.path(db).file_name() == Some("__init__.py") {
        ModuleKind::PackageInit
    } else {
        ModuleKind::Module
    };
    extract_models_impl(module.body(db), &module_name, file, module_kind)
}

mod discovery;
mod extract;
mod graph;
mod resolve;

use std::collections::BTreeMap;

pub use discovery::model_modules;
use djls_source::File;
pub use graph::ModelGraph;
pub use graph::ModelId;

use crate::db::Db;
use crate::models::extract::ModelExtraction;
use crate::models::extract::extract_models_impl;
use crate::models::resolve::resolve_deferred_models;
use crate::project::Project;
use crate::python::PythonModuleName;
use crate::python::PythonParseResult;
use crate::python::import_bindings;
use crate::python::parse_python_module;

/// Compute a merged `ModelGraph` from discovered model sources.
#[salsa::tracked(returns(ref))]
pub fn compute_model_graph(db: &dyn Db, project: Project) -> ModelGraph {
    // `ModelGraph::merge` is last-wins. Iterate search paths in reverse so
    // earlier Python search paths keep normal import precedence.
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
    let mut graph = ModelGraph::new();
    let mut import_bindings_by_module = BTreeMap::new();
    let mut deferred = Vec::new();

    for (file, module_name) in modules {
        let bindings = import_bindings(db, file, module_name.clone());
        import_bindings_by_module.insert(module_name.clone(), bindings);

        let extraction = extract_models(db, file, module_name);
        if !extraction.graph.is_empty() {
            graph.merge(extraction.graph.clone());
        }
        deferred.extend(extraction.deferred.iter().cloned());
    }

    resolve_deferred_models(db, project, &mut graph, deferred);
    graph.resolve_relation_targets(db, project, &import_bindings_by_module);
    #[cfg(debug_assertions)]
    graph.debug_assert_no_file_local_placeholders();
    graph
}

/// Extract models from one Python file, cached by Salsa.
///
/// This is a separate query from `compute_model_graph` so project-wide graph
/// recomputation can reuse unchanged per-file data.
#[salsa::tracked(returns(ref))]
pub fn extract_models(
    db: &dyn djls_source::Db,
    file: File,
    module_name: PythonModuleName,
) -> ModelExtraction {
    let PythonParseResult::Parsed(parsed) = parse_python_module(db, file) else {
        return ModelExtraction::default();
    };

    let imports = import_bindings(db, file, module_name.clone());
    extract_models_impl(parsed.body(db), module_name, file, imports)
}

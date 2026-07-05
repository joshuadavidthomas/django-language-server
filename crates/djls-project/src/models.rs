mod discovery;
mod extract;
mod graph;

use std::collections::BTreeMap;
use std::ops::ControlFlow;

pub use discovery::model_modules;
use djls_source::File;
pub use graph::ModelGraph;
pub use graph::ModelId;

use crate::ast::Recurse;
use crate::ast::walk_stmts;
use crate::db::Db;
use crate::models::extract::ModelCollector;
use crate::models::extract::resolve_children;
use crate::project::Project;
use crate::python::PythonModuleName;
use crate::python::import_table;
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
    let mut import_tables = BTreeMap::new();

    for (file, module_name) in modules {
        let table = import_table(db, file, module_name.clone());
        import_tables.insert(module_name.clone(), table);

        let model_graph = extract_model_graph(db, file, module_name);
        if !model_graph.is_empty() {
            graph.merge(model_graph.clone());
        }
    }

    graph.resolve_relation_targets(db, project, &import_tables);
    #[cfg(debug_assertions)]
    graph.debug_assert_no_file_local_placeholders();
    graph
}

/// Extract a model graph from one Python file, cached by Salsa.
///
/// This is a separate query from `compute_model_graph` so project-wide graph
/// recomputation can reuse unchanged per-file graphs.
#[salsa::tracked(returns(ref))]
pub fn extract_model_graph(
    db: &dyn djls_source::Db,
    file: File,
    module_name: PythonModuleName,
) -> ModelGraph {
    let source = file.source(db);
    let Some(parsed) = parse_python_module(db, file) else {
        return ModelGraph::default();
    };

    let imports = import_table(db, file, module_name.clone());
    extract_model_graph_impl(source.as_ref(), parsed.body(db), module_name, imports)
}

fn extract_model_graph_impl(
    source: &str,
    stmts: &[ruff_python_ast::Stmt],
    module_name: PythonModuleName,
    imports: &crate::python::ImportTable,
) -> ModelGraph {
    let mut collector = ModelCollector::new(module_name, source, imports);
    walk_stmts(stmts, Recurse::Flat, |stmt| {
        collector.scan_stmt(stmt);
        ControlFlow::Continue(())
    });
    resolve_children(
        &mut collector.graph,
        &collector.children,
        &collector.module_name,
        source,
    );
    collector.graph
}

mod extract;
mod graph;

use std::ops::ControlFlow;

use djls_source::File;
use djls_source::FileKind;
pub use graph::ModelGraph;
pub use graph::ModelId;

use crate::ast::Recurse;
use crate::ast::walk_stmts;
use crate::db::Db;
use crate::models::extract::ModelCollector;
use crate::project::Project;
use crate::python::PythonModuleName;
use crate::resolve::model_modules;

/// Compute a merged `ModelGraph` from discovered model sources.
#[salsa::tracked(returns(ref))]
pub fn compute_model_graph(db: &dyn Db, project: Project) -> ModelGraph {
    let mut graph = ModelGraph::new();

    // `ModelGraph::merge` is last-wins. Iterate search paths in reverse so
    // earlier Python search paths keep normal import precedence.
    for module in model_modules(db, project).iter().rev() {
        let model_graph = extract_model_graph(db, module.file(), module.name().clone());
        if !model_graph.is_empty() {
            graph.merge(model_graph.clone());
        }
    }

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
    if *source.kind() != FileKind::Python {
        return ModelGraph::default();
    }

    extract_model_graph_impl(source.as_ref(), module_name)
}

fn extract_model_graph_impl(source: &str, module_name: PythonModuleName) -> ModelGraph {
    let Ok(parsed) = ruff_python_parser::parse_module(source) else {
        return ModelGraph::default();
    };

    let module = parsed.into_syntax();
    let mut collector = ModelCollector::new(module_name, source);
    walk_stmts(&module.body, Recurse::Flat, |stmt| {
        collector.scan_stmt(stmt);
        ControlFlow::Continue(())
    });
    collector.finish()
}

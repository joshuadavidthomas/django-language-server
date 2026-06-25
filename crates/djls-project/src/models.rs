mod extract;
mod graph;

use djls_source::File;
pub use extract::extract_model_graph;
pub use graph::ModelGraph;

use crate::db::Db;
use crate::models::extract::extract_model_graph_from_body;
use crate::names::ModulePath;
use crate::parse::parse_python_module;
use crate::project::Project;
use crate::resolve::model_modules;

/// Compute a merged `ModelGraph` from discovered model sources.
#[salsa::tracked(returns(ref))]
pub fn compute_model_graph(db: &dyn Db, project: Project) -> ModelGraph {
    let mut graph = ModelGraph::new();

    // `ModelGraph::merge` is last-wins. Iterate search paths in reverse so
    // earlier Python search paths keep normal import precedence.
    for module in model_modules(db, project).iter().rev() {
        let model_graph =
            extract_model_graph_from_file(db, module.file(), module.module_path().clone());
        if !model_graph.is_empty() {
            graph.merge(model_graph.clone());
        }
    }

    graph
}

#[salsa::tracked(returns(ref))]
fn extract_model_graph_from_file(db: &dyn Db, file: File, module_path: ModulePath) -> ModelGraph {
    let source = file.source(db);
    let Some(parsed) = parse_python_module(db, file) else {
        return ModelGraph::default();
    };

    let module_path = module_path.into_string();
    extract_model_graph_from_body(parsed.body(db), source.as_ref(), &module_path)
}

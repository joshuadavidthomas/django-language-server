use djls_source::File;

use crate::db::Db;
use crate::filters::FilterAritySpecs;
use crate::project::Project;
use crate::project::project_model_modules;
use crate::project::project_templatetag_modules;
use crate::python::BlockSpecs;
use crate::python::FilterArityMap;
use crate::python::ModelGraph;
use crate::python::ModulePath;
use crate::python::TagRuleMap;
use crate::python::extract_block_specs;
use crate::python::extract_filter_arities;
use crate::python::extract_model_graph;
use crate::python::extract_tag_rules;
use crate::tags::TagSpecs;
use crate::tags::builtin_tag_specs;

/// Compute `TagSpecs` from tag-rule and block-spec extraction results.
///
/// This tracked function reads only the extraction domains needed to build tag
/// specs. Filter-only extraction changes should not invalidate this query.
///
/// Does NOT read from `Arc<Mutex<Settings>>`.
#[salsa::tracked(returns(ref))]
pub fn compute_tag_specs(db: &dyn Db, project: Project) -> TagSpecs {
    let _libraries = project.template_libraries(db);
    let tagspecs = project.tagspecs(db);

    let mut specs = builtin_tag_specs();

    let workspace_block_specs = collect_workspace_block_specs(db, project);
    for (_module_path, block_specs) in workspace_block_specs {
        specs.merge_block_specs(block_specs);
    }
    let workspace_tag_rules = collect_workspace_tag_rules(db, project);
    for (_module_path, tag_rules) in workspace_tag_rules {
        specs.merge_tag_rules(tag_rules);
    }

    for block_specs in project.extracted_external_block_specs(db).values() {
        specs.merge_block_specs(block_specs);
    }
    for tag_rules in project.extracted_external_tag_rules(db).values() {
        specs.merge_tag_rules(tag_rules);
    }

    if !tagspecs.libraries.is_empty() {
        let fallback = TagSpecs::from_tagspec_def(tagspecs);
        specs.merge_fallback(fallback);
    }

    specs
}

/// Compute `FilterAritySpecs` from a project's extraction results.
///
/// Merges filter arity data from both workspace and external extraction results,
/// with last-wins semantics for name collisions.
#[salsa::tracked(returns(ref))]
pub fn compute_filter_arity_specs(db: &dyn Db, project: Project) -> FilterAritySpecs {
    let mut specs = FilterAritySpecs::new();

    let workspace_results = collect_workspace_filter_arities(db, project);
    for (_module_path, filter_arities) in workspace_results {
        specs.merge_filter_arities(filter_arities);
    }

    for filter_arities in project.extracted_external_filter_arities(db).values() {
        specs.merge_filter_arities(filter_arities);
    }

    specs
}

/// Compute a merged `ModelGraph` from workspace and external model sources.
#[salsa::tracked(returns(ref))]
pub fn compute_model_graph(db: &dyn Db, project: Project) -> ModelGraph {
    let mut graph = ModelGraph::new();

    for model_graph in project.extracted_external_models(db).values() {
        graph.merge(model_graph.clone());
    }

    for (_module_path, model_graph) in collect_workspace_models(db, project) {
        graph.merge(model_graph.clone());
    }

    graph
}

#[salsa::tracked(returns(ref))]
fn collect_workspace_models(db: &dyn Db, project: Project) -> Vec<(ModulePath, ModelGraph)> {
    let mut results = Vec::new();

    for module in project_model_modules(db, project) {
        let graph = extract_workspace_model_graph(db, module.file(), module.module_path().clone());
        if !graph.is_empty() {
            results.push((module.module_path().clone(), graph));
        }
    }

    results
}

#[salsa::tracked]
fn extract_workspace_model_graph(db: &dyn Db, file: File, module_path: ModulePath) -> ModelGraph {
    let source = file.source(db);
    let module_path = module_path.into_string();
    extract_model_graph(source.as_ref(), &module_path)
}

#[salsa::tracked(returns(ref))]
fn collect_workspace_tag_rules(db: &dyn Db, project: Project) -> Vec<(String, TagRuleMap)> {
    let mut results = Vec::new();

    for module in project_templatetag_modules(db, project) {
        let file = module.file();
        let tag_rules = extract_tag_rules(db, file, module.module_path().clone());

        if !tag_rules.is_empty() {
            results.push((module.module_path().as_str().to_string(), tag_rules.clone()));
        }
    }

    results
}

#[salsa::tracked(returns(ref))]
fn collect_workspace_filter_arities(
    db: &dyn Db,
    project: Project,
) -> Vec<(String, FilterArityMap)> {
    let mut results = Vec::new();

    for module in project_templatetag_modules(db, project) {
        let file = module.file();
        let filter_arities = extract_filter_arities(db, file, module.module_path().clone());

        if !filter_arities.is_empty() {
            results.push((
                module.module_path().as_str().to_string(),
                filter_arities.clone(),
            ));
        }
    }

    results
}

#[salsa::tracked(returns(ref))]
fn collect_workspace_block_specs(db: &dyn Db, project: Project) -> Vec<(String, BlockSpecs)> {
    let mut results = Vec::new();

    for module in project_templatetag_modules(db, project) {
        let file = module.file();
        let block_specs = extract_block_specs(db, file, module.module_path().clone());

        if !block_specs.is_empty() {
            results.push((
                module.module_path().as_str().to_string(),
                block_specs.clone(),
            ));
        }
    }

    results
}

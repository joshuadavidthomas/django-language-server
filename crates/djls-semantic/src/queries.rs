use djls_source::File;

use crate::db::Db;
use crate::project::Project;
use crate::python::extract_block_specs;
use crate::python::extract_filter_arities;
use crate::python::extract_model_graph;
use crate::python::extract_tag_rules;
use crate::python::BlockSpecs;
use crate::python::FilterArityMap;
use crate::python::ModelGraph;
use crate::python::ModulePath;
use crate::python::TagRuleMap;
use crate::specs::filters::FilterAritySpecs;
use crate::specs::tags::builtin_tag_specs;
use crate::specs::tags::TagSpecs;

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

    let project_facts = djls_project::Db::project(db);
    for env in ready_static_environments(db, project) {
        for module in djls_project::python_module_inventory(db, project_facts, env)
            .modules()
            .iter()
            .filter(|module| module.has_role(djls_project::PythonModuleRole::Model))
        {
            let module_path = ModulePath::new(module.module().as_str().to_string());
            if results.iter().any(|(existing, _)| existing == &module_path) {
                continue;
            }
            let graph = extract_workspace_model_graph(db, module.file(), module_path.clone());
            if !graph.is_empty() {
                results.push((module_path, graph));
            }
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

    let project_facts = djls_project::Db::project(db);
    for env in ready_static_environments(db, project) {
        for module in djls_project::python_module_inventory(db, project_facts, env)
            .modules()
            .iter()
            .filter(|module| module.has_role(djls_project::PythonModuleRole::TemplateTag))
        {
            let module_name = module.module().as_str().to_string();
            if results.iter().any(|(existing, _)| existing == &module_name) {
                continue;
            }
            let file = module.file();
            let module_path = ModulePath::new(module_name.clone());
            let tag_rules = extract_tag_rules(db, file, module_path);

            if !tag_rules.is_empty() {
                results.push((module_name, tag_rules.clone()));
            }
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

    let project_facts = djls_project::Db::project(db);
    for env in ready_static_environments(db, project) {
        for module in djls_project::python_module_inventory(db, project_facts, env)
            .modules()
            .iter()
            .filter(|module| module.has_role(djls_project::PythonModuleRole::TemplateTag))
        {
            let module_name = module.module().as_str().to_string();
            if results.iter().any(|(existing, _)| existing == &module_name) {
                continue;
            }
            let file = module.file();
            let module_path = ModulePath::new(module_name.clone());
            let filter_arities = extract_filter_arities(db, file, module_path);

            if !filter_arities.is_empty() {
                results.push((module_name, filter_arities.clone()));
            }
        }
    }

    results
}

#[salsa::tracked(returns(ref))]
fn collect_workspace_block_specs(db: &dyn Db, project: Project) -> Vec<(String, BlockSpecs)> {
    let mut results = Vec::new();

    let project_facts = djls_project::Db::project(db);
    for env in ready_static_environments(db, project) {
        for module in djls_project::python_module_inventory(db, project_facts, env)
            .modules()
            .iter()
            .filter(|module| module.has_role(djls_project::PythonModuleRole::TemplateTag))
        {
            let module_name = module.module().as_str().to_string();
            if results.iter().any(|(existing, _)| existing == &module_name) {
                continue;
            }
            let file = module.file();
            let module_path = ModulePath::new(module_name.clone());
            let block_specs = extract_block_specs(db, file, module_path);

            if !block_specs.is_empty() {
                results.push((module_name, block_specs.clone()));
            }
        }
    }

    results
}

fn ready_static_environments(
    db: &dyn Db,
    legacy_project: Project,
) -> Vec<djls_project::DjangoEnvironmentId> {
    let project = djls_project::Db::project(db);
    let (djls_project::DjangoEnvironmentCandidatesOutcome::Ready { candidates, .. }
    | djls_project::DjangoEnvironmentCandidatesOutcome::Ambiguous { candidates, .. }) =
        djls_project::django_environment_candidates(db, project)
    else {
        return Vec::new();
    };
    let legacy_root = legacy_project.root(db);
    candidates
        .iter()
        .filter(|candidate| candidate.root().is_none_or(|root| root == legacy_root))
        .map(|candidate| candidate.id().clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;
    use djls_conf::Settings;
    use djls_project::testing::manage_py_path;
    use djls_project::testing::package_init_path;
    use djls_project::testing::project_discovery_set_for_test;
    use djls_project::testing::ready_source_inventory_with_roots_for_test;
    use djls_project::testing::settings_file_path;
    use djls_project::Db as ProjectFactsDb;
    use djls_project::ProjectDiscovery;

    use super::*;
    use crate::project::Project;
    use crate::testing::TestDatabase;

    #[test]
    fn queries_extract_models_from_static_python_module_inventory() {
        let mut db = TestDatabase::new();
        let root = Utf8PathBuf::from("/workspace");
        db.add_file(
            "/workspace/config/settings.py",
            "INSTALLED_APPS = ['blog']\n",
        );
        db.add_file("/workspace/blog/__init__.py", "");
        db.add_file(
            "/workspace/blog/models.py",
            "from django.db import models\nclass Post(models.Model):\n    pass\n",
        );
        db.set_project_source_inventory(ready_source_inventory_with_roots_for_test(
            &db,
            vec![root.clone()],
            vec![
                manage_py_path(&root),
                package_init_path(&root, "config"),
                settings_file_path(&root, "config"),
                package_init_path(&root, "blog"),
                root.join("blog/models.py"),
            ],
        ));
        db.set_project_discovery(ProjectDiscovery::Ready(project_discovery_set_for_test(
            &db,
            root.clone(),
        )));
        let legacy_project = Project::bootstrap(&db, root.as_path(), &Settings::default());

        let graph = compute_model_graph(&db, legacy_project);

        assert!(graph.get("Post").is_some());
    }
}

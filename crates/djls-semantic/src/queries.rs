use djls_source::File;

use crate::db::Db;
use crate::python::extract_block_specs;
use crate::python::extract_filter_arities;
use crate::python::extract_model_graph;
use crate::python::extract_tag_rules;
use crate::python::ModelGraph;
use crate::python::ModulePath;
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
pub fn compute_tag_specs(db: &dyn Db, project: djls_project::Project) -> TagSpecs {
    let tagspecs = project.tag_specs_config(db);

    let mut specs = builtin_tag_specs();

    for module in djls_project::template_tag_modules(db, project) {
        let module_path = ModulePath::new(module.module().as_str().to_string());
        let block_specs = extract_block_specs(db, module.file(), module_path.clone());
        if !block_specs.is_empty() {
            specs.merge_block_specs(block_specs);
        }

        let tag_rules = extract_tag_rules(db, module.file(), module_path);
        if !tag_rules.is_empty() {
            specs.merge_tag_rules(tag_rules);
        }
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
pub fn compute_filter_arity_specs(db: &dyn Db, project: djls_project::Project) -> FilterAritySpecs {
    let mut specs = FilterAritySpecs::new();

    for module in djls_project::template_tag_modules(db, project) {
        let module_path = ModulePath::new(module.module().as_str().to_string());
        let filter_arities = extract_filter_arities(db, module.file(), module_path);
        if !filter_arities.is_empty() {
            specs.merge_filter_arities(filter_arities);
        }
    }

    specs
}

/// Compute a merged `ModelGraph` from workspace and external model sources.
#[salsa::tracked(returns(ref))]
pub fn compute_model_graph(db: &dyn Db, project: djls_project::Project) -> ModelGraph {
    let mut graph = ModelGraph::new();

    for module in djls_project::model_modules(db, project) {
        let module_path = ModulePath::new(module.module().as_str().to_string());
        let model_graph = extract_workspace_model_graph(db, module.file(), module_path);
        if !model_graph.is_empty() {
            graph.merge(model_graph.clone());
        }
    }

    graph
}

#[salsa::tracked]
fn extract_workspace_model_graph(db: &dyn Db, file: File, module_path: ModulePath) -> ModelGraph {
    let source = file.source(db);
    let module_path = module_path.into_string();
    extract_model_graph(source.as_ref(), &module_path)
}

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;
    use djls_project::manage_py_path;
    use djls_project::package_init_path;
    use djls_project::project_roots_for_test;
    use djls_project::ready_source_inventory_with_roots_for_test;
    use djls_project::settings_file_path;
    use djls_project::Db as ProjectDb;
    use djls_project::ProjectRootDiscovery;

    use super::*;
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
        db.set_source_file_inventory(ready_source_inventory_with_roots_for_test(
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
        db.set_project_root_discovery(ProjectRootDiscovery::Ready(project_roots_for_test(
            &db,
            root.clone(),
        )));
        let project = djls_project::Db::project(&db);

        let graph = compute_model_graph(&db, project);

        assert!(graph.get("Post").is_some());
    }
}

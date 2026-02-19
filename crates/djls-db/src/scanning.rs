use camino::Utf8PathBuf;
use djls_project::Db as ProjectDb;
use djls_project::ResolvedModule;
use djls_python::ExtractionResult;
use djls_python::ModelGraph;
use djls_python::ModulePath;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;
use salsa::Setter;

/// Read discovered model files and extract model graphs.
///
/// Takes a list of `(module_path, file_path)` pairs (produced by a discovery
/// function in `djls-project`) and returns a map of module path to extracted
/// model graph, skipping files that fail to read or produce empty graphs.
fn extract_models_from_files(
    files: &[(ModulePath, Utf8PathBuf)],
) -> FxHashMap<ModulePath, ModelGraph> {
    let mut results = FxHashMap::default();

    for (module_path, file_path) in files {
        let source = match std::fs::read_to_string(file_path.as_std_path()) {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!("Failed to read model file {}: {}", file_path, e);
                continue;
            }
        };

        let graph = djls_python::extract_model_graph(&source, module_path.as_str());
        if !graph.is_empty() {
            results.insert(module_path.clone(), graph);
        }
    }

    results
}

/// Read resolved external module files and extract validation rules.
///
/// Takes resolved module metadata, reads each file from disk, and runs
/// rule extraction. Skips files that fail to read or produce empty results.
fn extract_rules_from_modules(modules: Vec<ResolvedModule>) -> FxHashMap<String, ExtractionResult> {
    let mut results = FxHashMap::default();

    for resolved in modules {
        match std::fs::read_to_string(resolved.file_path.as_std_path()) {
            Ok(source) => {
                let module_result = djls_python::extract_rules(&source, &resolved.module_path);
                if !module_result.is_empty() {
                    results.insert(resolved.module_path, module_result);
                }
            }
            Err(e) => {
                tracing::debug!("Failed to read module file {}: {}", resolved.file_path, e);
            }
        }
    }

    results
}

/// Scan the venv's site-packages for `models.py` files and extract model
/// graphs. Updates the project's `extracted_external_models` field if the
/// results differ from the current value.
///
/// Workspace `models.py` files are handled separately by
/// `collect_workspace_models` which uses tracked Salsa queries for
/// automatic invalidation on file change.
pub(crate) fn scan_external_models(db: &mut dyn ProjectDb) {
    let Some(project) = db.project() else {
        return;
    };

    let interpreter = project.interpreter(db).clone();
    let root = project.root(db).clone();

    let new_models = match djls_project::find_site_packages(&interpreter, &root) {
        Some(site_packages) => {
            let files = djls_project::discover_model_files_in_dir(&site_packages);
            extract_models_from_files(&files)
        }
        None => FxHashMap::default(),
    };

    if project.extracted_external_models(db) != &new_models {
        project.set_extracted_external_models(db).to(new_models);
    }
}

/// Extract validation rules from external (non-workspace) registration modules
/// and update the project's extracted rules if they differ.
///
/// Workspace modules are handled separately by `collect_workspace_extraction_results`
/// which uses tracked Salsa queries for automatic invalidation on file change.
pub(crate) fn scan_external_rules(db: &mut dyn ProjectDb) {
    let Some(project) = db.project() else {
        return;
    };

    let interpreter = project.interpreter(db).clone();
    let root = project.root(db).clone();
    let pythonpath = project.pythonpath(db).clone();

    let modules: FxHashSet<String> = project
        .template_libraries(db)
        .registration_modules()
        .into_iter()
        .map(|m| m.as_str().to_string())
        .collect();

    let new_extraction = if modules.is_empty() {
        FxHashMap::default()
    } else {
        let search_paths = djls_project::build_search_paths(&interpreter, &root, &pythonpath);
        let (_workspace, external_modules) =
            djls_project::resolve_modules(modules.iter().map(String::as_str), &search_paths, &root);
        extract_rules_from_modules(external_modules)
    };

    if project.extracted_external_rules(db) != &new_extraction {
        project.set_extracted_external_rules(db).to(new_extraction);
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;

    use super::*;

    #[test]
    fn extract_models_finds_models() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

        let app_dir = root.join("myapp");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            app_dir.join("models.py"),
            r"
from django.db import models

class Article(models.Model):
    title = models.CharField(max_length=200)
    author = models.ForeignKey('auth.User', on_delete=models.CASCADE)
",
        )
        .unwrap();

        let files = djls_project::discover_model_files_in_dir(&root);
        let results = extract_models_from_files(&files);
        assert_eq!(results.len(), 1);
        assert!(results.contains_key("myapp.models"));
        assert!(results["myapp.models"].get("Article").is_some());
    }

    #[test]
    fn extract_models_skips_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

        let app_dir = root.join("emptyapp");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(app_dir.join("models.py"), "# no models here\n").unwrap();

        let files = djls_project::discover_model_files_in_dir(&root);
        let results = extract_models_from_files(&files);
        assert!(results.is_empty());
    }

    #[test]
    fn extract_models_nested_apps() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

        for app in &["blog", "accounts"] {
            let app_dir = root.join(app);
            std::fs::create_dir_all(&app_dir).unwrap();
            std::fs::write(
                app_dir.join("models.py"),
                format!(
                    "from django.db import models\nclass {name}Model(models.Model):\n    pass\n",
                    name = app.chars().next().unwrap().to_uppercase().to_string() + &app[1..]
                ),
            )
            .unwrap();
        }

        let files = djls_project::discover_model_files_in_dir(&root);
        let results = extract_models_from_files(&files);
        assert_eq!(results.len(), 2);
        assert!(results.contains_key("blog.models"));
        assert!(results.contains_key("accounts.models"));
    }

    #[test]
    fn extract_models_package() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

        let models_dir = root.join("myapp/models");
        std::fs::create_dir_all(&models_dir).unwrap();
        std::fs::write(
            models_dir.join("__init__.py"),
            "from .user import User\nfrom .order import Order\n",
        )
        .unwrap();
        std::fs::write(
            models_dir.join("user.py"),
            "from django.db import models\nclass User(models.Model):\n    pass\n",
        )
        .unwrap();
        std::fs::write(
            models_dir.join("order.py"),
            "from django.db import models\nclass Order(models.Model):\n    user = models.ForeignKey(User, on_delete=models.CASCADE)\n",
        )
        .unwrap();

        let files = djls_project::discover_model_files_in_dir(&root);
        let results = extract_models_from_files(&files);
        // __init__.py has no model defs, so only the two submodules
        assert_eq!(results.len(), 2);
        assert!(results.contains_key("myapp.models.user"));
        assert!(results.contains_key("myapp.models.order"));
        assert!(results["myapp.models.user"].get("User").is_some());
        assert!(results["myapp.models.order"].get("Order").is_some());
    }

    #[test]
    fn extract_models_nested_package() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

        let base_dir = root.join("myapp/models/base");
        std::fs::create_dir_all(&base_dir).unwrap();
        std::fs::write(root.join("myapp/models/__init__.py"), "").unwrap();
        std::fs::write(base_dir.join("__init__.py"), "").unwrap();
        std::fs::write(
            base_dir.join("abstract.py"),
            "from django.db import models\nclass BaseModel(models.Model):\n    class Meta:\n        abstract = True\n",
        )
        .unwrap();

        let files = djls_project::discover_model_files_in_dir(&root);
        let results = extract_models_from_files(&files);
        assert!(
            results.contains_key("myapp.models.base.abstract"),
            "should extract from nested model files: got {:?}",
            results.keys().collect::<Vec<_>>()
        );
        assert!(results["myapp.models.base.abstract"]
            .get("BaseModel")
            .is_some());
    }
}

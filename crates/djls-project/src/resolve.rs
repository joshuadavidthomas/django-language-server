//! Module path → file path resolution using `sys_path`.

use camino::Utf8Path;
use camino::Utf8PathBuf;

use crate::Interpreter;

/// Derive a dotted module path from a relative filesystem path.
///
/// Strips the file extension and joins path components with dots.
/// For `__init__.py`, drops the `__init__` component to yield the
/// package path. For example:
/// - `myapp/models.py` → `myapp.models`
/// - `myapp/models/__init__.py` → `myapp.models`
/// - `myapp/models/user.py` → `myapp.models.user`
fn module_path_from_relative(rel: &Utf8Path) -> String {
    let without_ext = rel.with_extension("");
    let parts: Vec<&str> = without_ext.components().map(|c| c.as_str()).collect();
    if parts.last() == Some(&"__init__") {
        parts[..parts.len() - 1].join(".")
    } else {
        parts.join(".")
    }
}

/// Check whether a file path is a Django model source file.
///
/// Matches both `models.py` (single-file) and `.py` files inside a
/// `models/` package (a directory named `models` containing `__init__.py`).
fn is_model_file(path: &Utf8Path) -> bool {
    if path.file_name() == Some("models.py") {
        return true;
    }
    if path.extension() == Some("py") {
        if let Some(parent) = path.parent() {
            if parent.file_name() == Some("models") {
                return parent.join("__init__.py").exists();
            }
        }
    }
    false
}

/// Classification of where a module lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleLocation {
    /// Module is in the project workspace (tracked as File)
    Workspace,
    /// Module is external (site-packages, stdlib, etc.)
    External,
}

/// Resolved module information.
#[derive(Debug, Clone)]
pub struct ResolvedModule {
    pub module_path: String,
    pub file_path: Utf8PathBuf,
    pub location: ModuleLocation,
}

/// Resolve a Python module path to a file path.
///
/// Searches `sys_path` entries in order (first match wins, matching Python's
/// import semantics). Classifies the resolved path as Workspace or External
/// based on whether it falls under `project_root`.
///
/// Returns `Some(ResolvedModule)` if found, `None` otherwise.
#[must_use]
pub fn resolve_module(
    module_path: &str,
    sys_path: &[Utf8PathBuf],
    project_root: &Utf8Path,
) -> Option<ResolvedModule> {
    let parts: Vec<&str> = module_path.split('.').collect();

    for sys_entry in sys_path {
        let mut candidate = sys_entry.clone();
        for part in &parts {
            candidate.push(part);
        }

        // Try as .py file
        let py_file = candidate.with_extension("py");
        if py_file.exists() {
            let location = classify_location(&py_file, project_root);
            return Some(ResolvedModule {
                module_path: module_path.to_string(),
                file_path: py_file,
                location,
            });
        }

        // Try as package/__init__.py
        let init_file = candidate.join("__init__.py");
        if init_file.exists() {
            let location = classify_location(&init_file, project_root);
            return Some(ResolvedModule {
                module_path: module_path.to_string(),
                file_path: init_file,
                location,
            });
        }
    }

    None
}

fn classify_location(path: &Utf8Path, project_root: &Utf8Path) -> ModuleLocation {
    if path.starts_with(project_root) {
        ModuleLocation::Workspace
    } else {
        ModuleLocation::External
    }
}

/// Resolve multiple module paths, partitioned by location.
///
/// Returns `(workspace_modules, external_modules)`.
pub fn resolve_modules<'a>(
    module_paths: impl IntoIterator<Item = &'a str>,
    sys_path: &[Utf8PathBuf],
    project_root: &Utf8Path,
) -> (Vec<ResolvedModule>, Vec<ResolvedModule>) {
    let mut workspace = Vec::new();
    let mut external = Vec::new();

    for module_path in module_paths {
        if let Some(resolved) = resolve_module(module_path, sys_path, project_root) {
            match resolved.location {
                ModuleLocation::Workspace => workspace.push(resolved),
                ModuleLocation::External => external.push(resolved),
            }
        }
    }

    (workspace, external)
}

/// Build a list of directories to search when resolving Python module paths.
///
/// Includes:
/// - The project root (for workspace modules)
/// - Explicit PYTHONPATH entries
/// - Site-packages from the virtual environment (if available)
#[must_use]
pub fn build_search_paths(
    interpreter: &Interpreter,
    root: &Utf8Path,
    pythonpath: &[String],
) -> Vec<Utf8PathBuf> {
    let mut paths = Vec::new();

    // Project root
    paths.push(root.to_path_buf());

    // Explicit PYTHONPATH entries
    for p in pythonpath {
        let path = Utf8PathBuf::from(p);
        if path.is_dir() {
            paths.push(path);
        }
    }

    // Site-packages from venv
    if let Some(site_packages) = find_site_packages(interpreter, root) {
        paths.push(site_packages);
    }

    paths
}

/// Find the site-packages directory for the given interpreter.
#[must_use]
pub fn find_site_packages(interpreter: &Interpreter, root: &Utf8Path) -> Option<Utf8PathBuf> {
    match interpreter {
        Interpreter::VenvPath(path) => find_site_packages_in_venv(Utf8Path::new(path)),
        Interpreter::Auto => {
            for dir in &[".venv", "venv", "env", ".env"] {
                let candidate = root.join(dir);
                if candidate.is_dir() {
                    return find_site_packages_in_venv(&candidate);
                }
            }
            None
        }
        Interpreter::InterpreterPath(_) => None,
    }
}

/// Find site-packages within a specific venv directory.
fn find_site_packages_in_venv(venv: &Utf8Path) -> Option<Utf8PathBuf> {
    let lib_dir = venv.join("lib");
    if !lib_dir.is_dir() {
        return None;
    }

    // On Linux/macOS: lib/pythonX.Y/site-packages
    if let Ok(entries) = std::fs::read_dir(lib_dir.as_std_path()) {
        fn parse_python_dir_version(name: &str) -> Option<(u32, u32)> {
            let suffix = name.strip_prefix("python")?;
            let mut parts = suffix.splitn(2, '.');
            let major = parts.next()?.parse::<u32>().ok()?;
            let minor_part = parts.next()?;

            let minor_digits: String = minor_part
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            if minor_digits.is_empty() {
                return None;
            }
            let minor = minor_digits.parse::<u32>().ok()?;

            Some((major, minor))
        }

        struct PythonLibCandidate {
            version: Option<(u32, u32)>,
            name: String,
            path: std::path::PathBuf,
        }

        let mut candidates: Vec<PythonLibCandidate> = Vec::new();

        for entry in entries.flatten() {
            if let Ok(file_type) = entry.file_type() {
                if !file_type.is_dir() {
                    continue;
                }
            }

            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if !name_str.starts_with("python") {
                continue;
            }

            let version = parse_python_dir_version(&name_str);
            candidates.push(PythonLibCandidate {
                version,
                name: name_str.to_string(),
                path: entry.path(),
            });
        }

        candidates.sort_by(|a, b| match (&a.version, &b.version) {
            (Some(a_v), Some(b_v)) => a_v.cmp(b_v).then_with(|| a.name.cmp(&b.name)),
            (Some(_), None) => std::cmp::Ordering::Greater,
            (None, Some(_)) => std::cmp::Ordering::Less,
            (None, None) => a.name.cmp(&b.name),
        });

        for candidate in candidates.into_iter().rev() {
            if let Ok(site_packages) =
                Utf8PathBuf::from_path_buf(candidate.path.join("site-packages"))
            {
                if site_packages.is_dir() {
                    return Some(site_packages);
                }
            }
        }
    }

    // On Windows: Lib/site-packages (capitalized)
    let lib_site = venv.join("Lib").join("site-packages");
    if lib_site.is_dir() {
        return Some(lib_site);
    }

    None
}

/// Discover model source files in a directory tree and extract model graphs.
///
/// Walks the directory recursively looking for both `models.py` files and
/// `.py` files inside `models/` packages (directories with `__init__.py`).
/// Derives the dotted module path from the filesystem path relative to
/// `base_dir` and extracts model definitions from each file.
///
/// Multiple files from the same models package produce separate entries
/// keyed by their full module path (e.g., `myapp.models`, `myapp.models.user`).
///
/// Skips files that fail to read and empty graphs.
fn scan_models_in_dir(
    base_dir: &Utf8Path,
) -> rustc_hash::FxHashMap<String, djls_python::models::ModelGraph> {
    let mut results = rustc_hash::FxHashMap::default();

    for entry in ignore::WalkBuilder::new(base_dir.as_std_path())
        .hidden(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|ft| ft.is_file()))
    {
        let Ok(path) = Utf8PathBuf::try_from(entry.into_path()) else {
            continue;
        };

        if !is_model_file(&path) {
            continue;
        }

        let Some(rel) = path.strip_prefix(base_dir).ok() else {
            continue;
        };

        let module_path = module_path_from_relative(rel);

        let Ok(source) = std::fs::read_to_string(path.as_std_path()) else {
            continue;
        };

        let graph = djls_python::models::extract_model_graph(&source, &module_path);
        if !graph.is_empty() {
            results.insert(module_path, graph);
        }
    }

    results
}

/// Extract model graphs from external (non-workspace) directories.
///
/// Scans site-packages for `models.py` files and extracts model graphs.
/// Workspace `models.py` files should use tracked Salsa queries instead.
#[must_use]
pub fn extract_external_models(
    interpreter: &Interpreter,
    root: &Utf8Path,
) -> rustc_hash::FxHashMap<String, djls_python::models::ModelGraph> {
    let Some(site_packages) = find_site_packages(interpreter, root) else {
        return rustc_hash::FxHashMap::default();
    };

    scan_models_in_dir(&site_packages)
}

/// Discover model source files in the workspace and return their resolved paths.
///
/// Walks the project root looking for `models.py` files and `.py` files
/// inside `models/` packages. Returns a list of `(module_path, file_path)`
/// pairs where `module_path` is the dotted module path relative to the
/// project root.
#[must_use]
pub fn discover_workspace_model_files(root: &Utf8Path) -> Vec<(String, Utf8PathBuf)> {
    let mut results = Vec::new();

    for entry in ignore::WalkBuilder::new(root.as_std_path())
        .hidden(true)
        .git_ignore(true)
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|ft| ft.is_file()))
    {
        let Ok(path) = Utf8PathBuf::try_from(entry.into_path()) else {
            continue;
        };

        if !is_model_file(&path) {
            continue;
        }

        // Skip anything in site-packages (external)
        if path.components().any(|c| c.as_str() == "site-packages") {
            continue;
        }

        let Some(rel) = path.strip_prefix(root).ok() else {
            continue;
        };

        let module_path = module_path_from_relative(rel);

        results.push((module_path, path));
    }

    results
}

/// Extract validation rules from external (non-workspace) registration modules.
///
/// Resolves the given module paths, filters to external-only, reads each
/// source file from disk, and runs extraction. Returns a per-module map.
///
/// Workspace modules should NOT be extracted this way — they use tracked
/// Salsa queries for automatic invalidation on file change.
pub fn extract_external_rules(
    modules: &std::collections::HashSet<String, impl std::hash::BuildHasher>,
    interpreter: &Interpreter,
    root: &Utf8Path,
    pythonpath: &[String],
) -> rustc_hash::FxHashMap<String, djls_python::ExtractionResult> {
    let search_paths = build_search_paths(interpreter, root, pythonpath);

    let (_workspace, external_modules) =
        resolve_modules(modules.iter().map(String::as_str), &search_paths, root);

    let mut results = rustc_hash::FxHashMap::default();

    for resolved in external_modules {
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

#[cfg(test)]
mod tests {
    use super::*;

    struct TestLayout {
        _project_tmp: tempfile::TempDir,
        _external_tmp: tempfile::TempDir,
        project_root: Utf8PathBuf,
        external_root: Utf8PathBuf,
    }

    fn setup_test_layout() -> TestLayout {
        let project_tmp = tempfile::TempDir::new().unwrap();
        let project_root = Utf8PathBuf::try_from(project_tmp.path().to_path_buf()).unwrap();

        // Create workspace module under project root
        let workspace_tags = project_root.join("myproject/templatetags");
        std::fs::create_dir_all(&workspace_tags).unwrap();
        std::fs::write(workspace_tags.join("custom.py"), "# workspace").unwrap();

        // Create external module in a SEPARATE temp dir (outside project root)
        let external_tmp = tempfile::TempDir::new().unwrap();
        let external_root = Utf8PathBuf::try_from(external_tmp.path().to_path_buf()).unwrap();
        let django_tags = external_root.join("django/templatetags");
        std::fs::create_dir_all(&django_tags).unwrap();
        std::fs::write(django_tags.join("i18n.py"), "# django").unwrap();

        TestLayout {
            _project_tmp: project_tmp,
            _external_tmp: external_tmp,
            project_root,
            external_root,
        }
    }

    #[test]
    fn resolve_workspace_module() {
        let layout = setup_test_layout();
        let sys_path = vec![layout.project_root.clone()];

        let result = resolve_module(
            "myproject.templatetags.custom",
            &sys_path,
            &layout.project_root,
        );

        assert!(result.is_some());
        let resolved = result.unwrap();
        assert_eq!(resolved.location, ModuleLocation::Workspace);
        assert!(resolved.file_path.ends_with("custom.py"));
    }

    #[test]
    fn resolve_external_module() {
        let layout = setup_test_layout();
        let sys_path = vec![layout.external_root.clone()];

        let result = resolve_module("django.templatetags.i18n", &sys_path, &layout.project_root);

        assert!(result.is_some());
        let resolved = result.unwrap();
        assert_eq!(resolved.location, ModuleLocation::External);
        assert!(resolved.file_path.ends_with("i18n.py"));
    }

    #[test]
    fn resolve_not_found() {
        let layout = setup_test_layout();
        let sys_path = vec![layout.project_root.clone()];

        let result = resolve_module("nonexistent.module", &sys_path, &layout.project_root);
        assert!(result.is_none());
    }

    #[test]
    fn sys_path_order_matters() {
        let layout = setup_test_layout();

        // Create same module in two places under project root
        let dir1 = layout.project_root.join("first");
        let dir2 = layout.project_root.join("second");
        std::fs::create_dir_all(dir1.join("pkg")).unwrap();
        std::fs::create_dir_all(dir2.join("pkg")).unwrap();
        std::fs::write(dir1.join("pkg/mod.py"), "# first").unwrap();
        std::fs::write(dir2.join("pkg/mod.py"), "# second").unwrap();

        // First in sys_path wins
        let sys_path = vec![dir1.clone(), dir2.clone()];
        let result = resolve_module("pkg.mod", &sys_path, &layout.project_root).unwrap();
        assert!(result.file_path.starts_with(&dir1));

        // Reverse order → different result
        let sys_path = vec![dir2.clone(), dir1.clone()];
        let result = resolve_module("pkg.mod", &sys_path, &layout.project_root).unwrap();
        assert!(result.file_path.starts_with(&dir2));
    }

    #[test]
    fn resolve_package_init() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

        // Create package with __init__.py
        let pkg_dir = root.join("myapp/templatetags/extras");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(pkg_dir.join("__init__.py"), "# package").unwrap();

        let sys_path = vec![root.clone()];
        let result = resolve_module("myapp.templatetags.extras", &sys_path, &root);

        assert!(result.is_some());
        let resolved = result.unwrap();
        assert!(resolved.file_path.ends_with("__init__.py"));
        assert_eq!(resolved.location, ModuleLocation::Workspace);
    }

    #[test]
    fn resolve_modules_partitions() {
        let layout = setup_test_layout();
        let sys_path = vec![layout.project_root.clone(), layout.external_root.clone()];

        let paths = [
            "myproject.templatetags.custom",
            "django.templatetags.i18n",
            "nonexistent.module",
        ];

        let (workspace, external) =
            resolve_modules(paths.iter().copied(), &sys_path, &layout.project_root);

        assert_eq!(workspace.len(), 1);
        assert_eq!(external.len(), 1);
        assert_eq!(workspace[0].module_path, "myproject.templatetags.custom");
        assert_eq!(external[0].module_path, "django.templatetags.i18n");
    }

    #[test]
    fn scan_models_in_dir_finds_models() {
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

        let results = scan_models_in_dir(&root);
        assert_eq!(results.len(), 1);
        assert!(results.contains_key("myapp.models"));
        let graph = &results["myapp.models"];
        assert!(graph.get("Article").is_some());
    }

    #[test]
    fn scan_models_in_dir_skips_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

        let app_dir = root.join("emptyapp");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(app_dir.join("models.py"), "# no models here\n").unwrap();

        let results = scan_models_in_dir(&root);
        assert!(results.is_empty());
    }

    #[test]
    fn scan_models_in_dir_nested_apps() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

        // Create two nested apps
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

        let results = scan_models_in_dir(&root);
        assert_eq!(results.len(), 2);
        assert!(results.contains_key("blog.models"));
        assert!(results.contains_key("accounts.models"));
    }

    #[test]
    fn discover_workspace_model_files_finds_models() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

        let app_dir = root.join("myapp");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            app_dir.join("models.py"),
            "from django.db import models\nclass Foo(models.Model): pass\n",
        )
        .unwrap();

        let results = discover_workspace_model_files(&root);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "myapp.models");
        assert!(results[0].1.ends_with("models.py"));
    }

    #[test]
    fn scan_models_package() {
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

        let results = scan_models_in_dir(&root);
        // __init__.py has no model defs, so only the two submodules
        assert_eq!(results.len(), 2);
        assert!(results.contains_key("myapp.models.user"));
        assert!(results.contains_key("myapp.models.order"));
        assert!(results["myapp.models.user"].get("User").is_some());
        assert!(results["myapp.models.order"].get("Order").is_some());
    }

    #[test]
    fn discover_workspace_models_package() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

        let models_dir = root.join("myapp/models");
        std::fs::create_dir_all(&models_dir).unwrap();
        std::fs::write(models_dir.join("__init__.py"), "").unwrap();
        std::fs::write(
            models_dir.join("user.py"),
            "from django.db import models\nclass User(models.Model): pass\n",
        )
        .unwrap();

        let results = discover_workspace_model_files(&root);
        let module_paths: Vec<&str> = results.iter().map(|(m, _)| m.as_str()).collect();
        assert!(
            module_paths.contains(&"myapp.models"),
            "should discover __init__.py as myapp.models"
        );
        assert!(
            module_paths.contains(&"myapp.models.user"),
            "should discover user.py as myapp.models.user"
        );
    }

    #[test]
    fn module_path_from_init_file() {
        let path = Utf8Path::new("myapp/models/__init__.py");
        assert_eq!(module_path_from_relative(path), "myapp.models");
    }

    #[test]
    fn module_path_from_submodule() {
        let path = Utf8Path::new("myapp/models/user.py");
        assert_eq!(module_path_from_relative(path), "myapp.models.user");
    }

    #[test]
    fn discover_workspace_model_files_skips_site_packages() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

        // Use a non-hidden venv path so the hidden filter doesn't mask
        // the site-packages component check we're actually testing.
        let sp = root.join("venv/lib/python3.12/site-packages/somelib");
        std::fs::create_dir_all(&sp).unwrap();
        std::fs::write(
            sp.join("models.py"),
            "from django.db import models\nclass Lib(models.Model): pass\n",
        )
        .unwrap();

        let results = discover_workspace_model_files(&root);
        assert!(
            results.is_empty(),
            "should not discover models in site-packages"
        );
    }
}

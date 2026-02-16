//! Module path → file path resolution using `sys_path`.

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_python::ModulePath;

use crate::Interpreter;

/// Derive a dotted module path from a relative filesystem path.
///
/// Strips the file extension and joins path components with dots.
/// For `__init__.py`, drops the `__init__` component to yield the
/// package path. For example:
/// - `myapp/models.py` → `myapp.models`
/// - `myapp/models/__init__.py` → `myapp.models`
/// - `myapp/models/user.py` → `myapp.models.user`
fn module_path_from_relative(rel: &Utf8Path) -> ModulePath {
    let without_ext = rel.with_extension("");
    let parts: Vec<&str> = without_ext.components().map(|c| c.as_str()).collect();
    let dotted = if parts.last() == Some(&"__init__") {
        parts[..parts.len() - 1].join(".")
    } else {
        parts.join(".")
    };
    ModulePath::new(dotted)
}

/// Check whether a file path is a Django model source file.
///
/// Matches `models.py` (single-file) and any `.py` file nested at any
/// depth inside a `models/` package (a directory named `models` that
/// contains `__init__.py`).
fn is_model_file(path: &Utf8Path) -> bool {
    if path.file_name() == Some("models.py") {
        return true;
    }
    if path.extension() == Some("py") {
        let mut dir = path.parent();
        while let Some(d) = dir {
            if d.file_name() == Some("models") {
                return d.join("__init__.py").exists();
            }
            dir = d.parent();
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

/// Shared walk-and-collect logic for model file discovery.
///
/// Configures a `WalkBuilder` via `builder_cfg`, walks the directory tree,
/// and collects `(module_path, file_path)` pairs for files that pass
/// `is_model_file` and the caller-supplied `filter` predicate.
fn discover_model_files(
    base_dir: &Utf8Path,
    builder_cfg: impl FnOnce(&mut ignore::WalkBuilder),
    filter: impl Fn(&Utf8Path) -> bool,
) -> Vec<(ModulePath, Utf8PathBuf)> {
    let mut builder = ignore::WalkBuilder::new(base_dir.as_std_path());
    builder_cfg(&mut builder);

    let mut results = Vec::new();

    for entry in builder
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|ft| ft.is_file()))
    {
        let Ok(path) = Utf8PathBuf::try_from(entry.into_path()) else {
            continue;
        };

        if !is_model_file(&path) || !filter(&path) {
            continue;
        }

        let Some(rel) = path.strip_prefix(base_dir).ok() else {
            continue;
        };

        results.push((module_path_from_relative(rel), path));
    }

    results.sort_by(|(a, _), (b, _)| a.cmp(b));
    results
}

/// Discover model source files in a directory tree and return their resolved paths.
///
/// Walks the directory recursively looking for both `models.py` files and
/// `.py` files inside `models/` packages (directories with `__init__.py`).
/// Returns `(module_path, file_path)` pairs without reading file contents.
///
/// Uses a raw walk with no git-ignore filtering, suitable for directories
/// outside the workspace (e.g. site-packages).
#[must_use]
pub fn discover_model_files_in_dir(base_dir: &Utf8Path) -> Vec<(ModulePath, Utf8PathBuf)> {
    discover_model_files(
        base_dir,
        |wb| {
            wb.hidden(false)
                .git_ignore(false)
                .git_global(false)
                .git_exclude(false);
        },
        |_| true,
    )
}

/// Discover model source files in the workspace and return their resolved paths.
///
/// Walks the project root looking for `models.py` files and `.py` files
/// inside `models/` packages. Returns a list of `(module_path, file_path)`
/// pairs where `module_path` is the dotted module path relative to the
/// project root.
#[must_use]
pub fn discover_workspace_model_files(root: &Utf8Path) -> Vec<(ModulePath, Utf8PathBuf)> {
    discover_model_files(
        root,
        |wb| {
            wb.hidden(true).git_ignore(true);
        },
        |path| !path.components().any(|c| c.as_str() == "site-packages"),
    )
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
    fn discover_model_files_in_dir_finds_models() {
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

        let results = discover_model_files_in_dir(&root);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0.as_str(), "myapp.models");
        assert!(results[0].1.ends_with("models.py"));
    }

    #[test]
    fn discover_model_files_in_dir_finds_files_without_inspecting_contents() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

        let app_dir = root.join("emptyapp");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(app_dir.join("models.py"), "# no models here\n").unwrap();

        // Discovery finds the file (it doesn't inspect contents)
        let results = discover_model_files_in_dir(&root);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0.as_str(), "emptyapp.models");
    }

    #[test]
    fn discover_model_files_in_dir_nested_apps() {
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

        let results = discover_model_files_in_dir(&root);
        assert_eq!(results.len(), 2);
        let module_paths: Vec<&str> = results.iter().map(|(m, _)| m.as_str()).collect();
        assert!(module_paths.contains(&"blog.models"));
        assert!(module_paths.contains(&"accounts.models"));
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
        assert_eq!(results[0].0.as_str(), "myapp.models");
        assert!(results[0].1.ends_with("models.py"));
    }

    #[test]
    fn discover_model_files_in_dir_package() {
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

        let results = discover_model_files_in_dir(&root);
        // Discovers all three files (including __init__.py)
        assert_eq!(results.len(), 3);
        let module_paths: Vec<&str> = results.iter().map(|(m, _)| m.as_str()).collect();
        assert!(module_paths.contains(&"myapp.models"));
        assert!(module_paths.contains(&"myapp.models.user"));
        assert!(module_paths.contains(&"myapp.models.order"));
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
        assert_eq!(module_path_from_relative(path).as_str(), "myapp.models");
    }

    #[test]
    fn module_path_from_submodule() {
        let path = Utf8Path::new("myapp/models/user.py");
        assert_eq!(
            module_path_from_relative(path).as_str(),
            "myapp.models.user"
        );
    }

    #[test]
    fn discover_workspace_models_nested_package() {
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

        let results = discover_workspace_model_files(&root);
        let module_paths: Vec<&str> = results.iter().map(|(m, _)| m.as_str()).collect();
        assert!(
            module_paths.contains(&"myapp.models.base.abstract"),
            "should discover nested model files: got {:?}",
            module_paths
        );
    }

    #[test]
    fn discover_model_files_in_dir_nested_package() {
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

        let results = discover_model_files_in_dir(&root);
        let module_paths: Vec<&str> = results.iter().map(|(m, _)| m.as_str()).collect();
        assert!(
            module_paths.contains(&"myapp.models.base.abstract"),
            "should discover nested model files: got {:?}",
            module_paths
        );
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

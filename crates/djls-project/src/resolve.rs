//! Module path → file path resolution using `sys_path`.

use camino::Utf8Path;
use camino::Utf8PathBuf;

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
#[must_use]
///
/// # Arguments
/// * `module_path` - Dotted module path (e.g., "django.templatetags.i18n")
/// * `sys_path` - Python sys.path entries to search
/// * `project_root` - Project root for workspace vs external classification
///
/// # Returns
/// `Some(ResolvedModule)` if found, `None` otherwise
pub fn resolve_module(
    module_path: &str,
    sys_path: &[Utf8PathBuf],
    project_root: &Utf8Path,
) -> Option<ResolvedModule> {
    let parts: Vec<&str> = module_path.split('.').collect();

    for sys_entry in sys_path {
        // Build candidate: {sys_entry}/a/b/c.py
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
pub fn resolve_modules(
    module_paths: impl IntoIterator<Item = impl AsRef<str>>,
    sys_path: &[Utf8PathBuf],
    project_root: &Utf8Path,
) -> (Vec<ResolvedModule>, Vec<ResolvedModule>) {
    let mut workspace = Vec::new();
    let mut external = Vec::new();

    for module_path in module_paths {
        if let Some(resolved) = resolve_module(module_path.as_ref(), sys_path, project_root) {
            match resolved.location {
                ModuleLocation::Workspace => workspace.push(resolved),
                ModuleLocation::External => external.push(resolved),
            }
        }
    }

    (workspace, external)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_layout() -> (TempDir, Utf8PathBuf, Utf8PathBuf) {
        let tmp = TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

        // Create workspace module
        let workspace_tags = root.join("myproject/templatetags");
        fs::create_dir_all(&workspace_tags).unwrap();
        fs::write(workspace_tags.join("custom.py"), "# workspace").unwrap();

        // Create external module (simulated site-packages) in a separate location
        // Use tmp.path().parent() to get outside the root
        let external_base = Utf8PathBuf::try_from(
            tmp.path().parent().unwrap().join("external_site_packages")
        ).unwrap();
        let django_tags = external_base.join("django/templatetags");
        fs::create_dir_all(&django_tags).unwrap();
        fs::write(django_tags.join("i18n.py"), "# django").unwrap();

        (tmp, root, external_base)
    }

    #[test]
    fn test_resolve_workspace_module() {
        let (_tmp, root, _external_base) = setup_test_layout();
        let sys_path = vec![root.clone()];

        let result = resolve_module("myproject.templatetags.custom", &sys_path, &root);

        assert!(result.is_some());
        let resolved = result.unwrap();
        assert_eq!(resolved.location, ModuleLocation::Workspace);
        assert!(resolved.file_path.ends_with("custom.py"));
    }

    #[test]
    fn test_resolve_external_module() {
        let (_tmp, root, external_base) = setup_test_layout();
        let sys_path = vec![external_base.clone()];

        // External modules are resolved relative to the project root
        let result = resolve_module("django.templatetags.i18n", &sys_path, &root);

        assert!(result.is_some());
        let resolved = result.unwrap();
        assert_eq!(resolved.location, ModuleLocation::External);
        assert!(resolved.file_path.ends_with("i18n.py"));
    }

    #[test]
    fn test_resolve_not_found() {
        let (_tmp, root, _external_base) = setup_test_layout();
        let sys_path = vec![root.clone()];

        let result = resolve_module("nonexistent.module", &sys_path, &root);
        assert!(result.is_none());
    }

    #[test]
    fn test_sys_path_order_matters() {
        let (_tmp, root, _external_base) = setup_test_layout();

        // Create same module in two places
        let dir1 = root.join("first");
        let dir2 = root.join("second");
        fs::create_dir_all(dir1.join("pkg")).unwrap();
        fs::create_dir_all(dir2.join("pkg")).unwrap();
        fs::write(dir1.join("pkg/mod.py"), "# first").unwrap();
        fs::write(dir2.join("pkg/mod.py"), "# second").unwrap();

        // First in sys_path wins
        let sys_path = vec![dir1.clone(), dir2.clone()];
        let result = resolve_module("pkg.mod", &sys_path, &root).unwrap();
        assert!(result.file_path.starts_with(&dir1));

        // Reverse order → different result
        let sys_path = vec![dir2.clone(), dir1.clone()];
        let result = resolve_module("pkg.mod", &sys_path, &root).unwrap();
        assert!(result.file_path.starts_with(&dir2));
    }

    #[test]
    fn test_resolve_package_init() {
        let tmp = TempDir::new().unwrap();
        let root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();

        // Create a package with __init__.py
        let pkg = root.join("mypackage/templatetags");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(pkg.join("__init__.py"), "# package").unwrap();

        let sys_path = vec![root.clone()];
        let result = resolve_module("mypackage.templatetags", &sys_path, &root);

        assert!(result.is_some());
        let resolved = result.unwrap();
        assert!(resolved.file_path.ends_with("__init__.py"));
    }

    #[test]
    fn test_resolve_modules_partitioning() {
        let (_tmp, root, external_base) = setup_test_layout();

        // Include both workspace and external paths
        let sys_path = vec![root.clone(), external_base];
        let module_paths = vec![
            "myproject.templatetags.custom",
            "django.templatetags.i18n", // found in external_base
        ];

        let (workspace, external) = resolve_modules(module_paths, &sys_path, &root);

        assert_eq!(workspace.len(), 1);
        assert_eq!(external.len(), 1);
        assert!(workspace[0].file_path.ends_with("custom.py"));
        assert!(external[0].file_path.ends_with("i18n.py"));
    }
}

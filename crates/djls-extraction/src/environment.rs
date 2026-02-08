use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

/// A template tag library discovered in the Python environment by scanning
/// `templatetags/` directories across `sys.path`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentLibrary {
    /// The load name used in `{% load X %}` (derived from filename stem).
    pub load_name: String,
    /// The dotted Python module path of the containing app
    /// (e.g., `django.contrib.humanize`).
    pub app_module: String,
    /// The dotted Python module path of the templatetags file
    /// (e.g., `django.contrib.humanize.templatetags.humanize`).
    pub module_path: String,
    /// Absolute path to the `templatetags/*.py` source file.
    pub source_path: PathBuf,
}

/// Inventory of all template tag libraries discovered in the Python environment.
///
/// Built by scanning `sys.path` entries for `*/templatetags/*.py` files.
/// This is a superset of the inspector inventory — it includes libraries from
/// apps that may not be in `INSTALLED_APPS`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentInventory {
    /// Map from load name → list of libraries (Vec because name collisions
    /// across packages are possible).
    libraries: BTreeMap<String, Vec<EnvironmentLibrary>>,
}

impl EnvironmentInventory {
    /// All discovered libraries, grouped by load name.
    #[must_use]
    pub fn libraries(&self) -> &BTreeMap<String, Vec<EnvironmentLibrary>> {
        &self.libraries
    }

    /// Whether a library with the given load name exists in the environment.
    #[must_use]
    pub fn has_library(&self, name: &str) -> bool {
        self.libraries.contains_key(name)
    }

    /// Get all libraries registered under a given load name.
    #[must_use]
    pub fn libraries_for_name(&self, name: &str) -> &[EnvironmentLibrary] {
        self.libraries
            .get(name)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// Total number of distinct load names.
    #[must_use]
    pub fn len(&self) -> usize {
        self.libraries.len()
    }

    /// Whether the inventory is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.libraries.is_empty()
    }
}

/// Scan Python environment paths to discover all template tag libraries.
///
/// Globs each `sys_path` entry for `*/templatetags/*.py`, skipping `__init__.py`
/// and `__pycache__` directories. Derives `load_name` from filename stem and
/// `app_module` from parent directory structure.
#[must_use]
pub fn scan_environment(sys_paths: &[PathBuf]) -> EnvironmentInventory {
    let mut libraries: BTreeMap<String, Vec<EnvironmentLibrary>> = BTreeMap::new();

    for sys_path in sys_paths {
        if !sys_path.is_dir() {
            continue;
        }
        scan_sys_path_entry(sys_path, &mut libraries);
    }

    EnvironmentInventory { libraries }
}

fn scan_sys_path_entry(
    sys_path: &Path,
    libraries: &mut BTreeMap<String, Vec<EnvironmentLibrary>>,
) {
    let Ok(top_entries) = std::fs::read_dir(sys_path) else {
        return;
    };

    for entry in top_entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        scan_package_tree(&path, sys_path, libraries);
    }
}

fn scan_package_tree(
    dir: &Path,
    sys_path: &Path,
    libraries: &mut BTreeMap<String, Vec<EnvironmentLibrary>>,
) {
    let templatetags_dir = dir.join("templatetags");
    if templatetags_dir.is_dir() {
        let init_file = templatetags_dir.join("__init__.py");
        if init_file.exists() {
            scan_templatetags_dir(&templatetags_dir, sys_path, libraries);
        }
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.')
            || name_str == "__pycache__"
            || name_str == "templatetags"
            || name_str.contains(".dist-info")
            || name_str.contains(".egg-info")
        {
            continue;
        }
        // Only recurse into directories that look like Python packages
        let init = path.join("__init__.py");
        if init.exists() {
            scan_package_tree(&path, sys_path, libraries);
        }
    }
}

fn scan_templatetags_dir(
    templatetags_dir: &Path,
    sys_path: &Path,
    libraries: &mut BTreeMap<String, Vec<EnvironmentLibrary>>,
) {
    let Ok(entries) = std::fs::read_dir(templatetags_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let Some(ext) = path.extension() else {
            continue;
        };
        if ext != "py" {
            continue;
        }

        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if stem == "__init__" {
            continue;
        }

        let load_name = stem.to_string();

        let Some(rel_path) = pathdiff(templatetags_dir.parent().unwrap(), sys_path) else {
            continue;
        };
        let app_module = path_to_dotted(&rel_path);

        let Some(full_rel) = pathdiff(&path, sys_path) else {
            continue;
        };
        let module_path = path_to_dotted_strip_py(&full_rel);

        let abs_path = if path.is_absolute() {
            path.clone()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(&path))
                .unwrap_or(path.clone())
        };

        let lib = EnvironmentLibrary {
            load_name: load_name.clone(),
            app_module,
            module_path,
            source_path: abs_path,
        };

        libraries.entry(load_name).or_default().push(lib);
    }
}

fn pathdiff(target: &Path, base: &Path) -> Option<PathBuf> {
    target.strip_prefix(base).ok().map(PathBuf::from)
}

fn path_to_dotted(rel_path: &Path) -> String {
    rel_path
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join(".")
}

fn path_to_dotted_strip_py(rel_path: &Path) -> String {
    let dotted = path_to_dotted(rel_path);
    dotted.strip_suffix(".py").unwrap_or(&dotted).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_templatetags_layout(
        root: &Path,
        packages: &[(&str, &[&str])],
    ) {
        for (package_path, tag_files) in packages {
            let parts: Vec<&str> = package_path.split('/').collect();
            let mut current = root.to_path_buf();

            // Create all package directories with __init__.py
            for part in &parts {
                current.push(part);
                std::fs::create_dir_all(&current).unwrap();
                std::fs::write(current.join("__init__.py"), "").unwrap();
            }

            // Create templatetags directory
            let templatetags_dir = current.join("templatetags");
            std::fs::create_dir_all(&templatetags_dir).unwrap();
            std::fs::write(templatetags_dir.join("__init__.py"), "").unwrap();

            for tag_file in *tag_files {
                std::fs::write(
                    templatetags_dir.join(format!("{tag_file}.py")),
                    "# templatetag module",
                )
                .unwrap();
            }
        }
    }

    #[test]
    fn scan_discovers_libraries() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        create_templatetags_layout(
            root,
            &[
                ("django/contrib/humanize", &["humanize"]),
                ("django/contrib/admin", &["admin_list", "admin_modify"]),
            ],
        );

        let inventory = scan_environment(&[root.to_path_buf()]);

        assert!(inventory.has_library("humanize"));
        assert!(inventory.has_library("admin_list"));
        assert!(inventory.has_library("admin_modify"));
        assert!(!inventory.has_library("__init__"));
    }

    #[test]
    fn scan_derives_correct_app_module() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        create_templatetags_layout(root, &[("django/contrib/humanize", &["humanize"])]);

        let inventory = scan_environment(&[root.to_path_buf()]);
        let libs = inventory.libraries_for_name("humanize");
        assert_eq!(libs.len(), 1);
        assert_eq!(libs[0].app_module, "django.contrib.humanize");
        assert_eq!(
            libs[0].module_path,
            "django.contrib.humanize.templatetags.humanize"
        );
    }

    #[test]
    fn scan_name_collision_detection() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        // Two packages with same templatetag filename
        create_templatetags_layout(
            root,
            &[("pkg_a", &["utils"]), ("pkg_b", &["utils"])],
        );

        let inventory = scan_environment(&[root.to_path_buf()]);
        let libs = inventory.libraries_for_name("utils");
        assert_eq!(libs.len(), 2);

        let app_modules: Vec<&str> = libs.iter().map(|l| l.app_module.as_str()).collect();
        assert!(app_modules.contains(&"pkg_a"));
        assert!(app_modules.contains(&"pkg_b"));
    }

    #[test]
    fn scan_skips_init_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        create_templatetags_layout(root, &[("myapp", &["custom"])]);

        let inventory = scan_environment(&[root.to_path_buf()]);
        assert!(!inventory.has_library("__init__"));
        assert!(inventory.has_library("custom"));
    }

    #[test]
    fn scan_requires_templatetags_init() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        // Create templatetags dir WITHOUT __init__.py
        let pkg_dir = root.join("myapp");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(pkg_dir.join("__init__.py"), "").unwrap();

        let tags_dir = pkg_dir.join("templatetags");
        std::fs::create_dir_all(&tags_dir).unwrap();
        // No __init__.py in templatetags
        std::fs::write(tags_dir.join("custom.py"), "# tag module").unwrap();

        let inventory = scan_environment(&[root.to_path_buf()]);
        assert!(!inventory.has_library("custom"));
    }

    #[test]
    fn scan_empty_directory() {
        let tmp = tempfile::TempDir::new().unwrap();
        let inventory = scan_environment(&[tmp.path().to_path_buf()]);
        assert!(inventory.is_empty());
    }

    #[test]
    fn scan_nonexistent_path() {
        let inventory = scan_environment(&[PathBuf::from("/nonexistent/path/12345")]);
        assert!(inventory.is_empty());
    }

    #[test]
    fn scan_multiple_sys_paths() {
        let tmp1 = tempfile::TempDir::new().unwrap();
        let tmp2 = tempfile::TempDir::new().unwrap();

        create_templatetags_layout(tmp1.path(), &[("pkg1", &["tags1"])]);
        create_templatetags_layout(tmp2.path(), &[("pkg2", &["tags2"])]);

        let inventory = scan_environment(&[
            tmp1.path().to_path_buf(),
            tmp2.path().to_path_buf(),
        ]);

        assert!(inventory.has_library("tags1"));
        assert!(inventory.has_library("tags2"));
        assert_eq!(inventory.len(), 2);
    }

    #[test]
    fn libraries_for_unknown_name_returns_empty() {
        let inventory = EnvironmentInventory::default();
        assert!(inventory.libraries_for_name("nonexistent").is_empty());
    }

    #[test]
    fn scan_skips_non_py_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        let pkg_dir = root.join("myapp");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(pkg_dir.join("__init__.py"), "").unwrap();

        let tags_dir = pkg_dir.join("templatetags");
        std::fs::create_dir_all(&tags_dir).unwrap();
        std::fs::write(tags_dir.join("__init__.py"), "").unwrap();
        std::fs::write(tags_dir.join("tags.py"), "# tag").unwrap();
        std::fs::write(tags_dir.join("readme.txt"), "# readme").unwrap();
        std::fs::write(tags_dir.join("data.json"), "{}").unwrap();

        let inventory = scan_environment(&[root.to_path_buf()]);
        assert_eq!(inventory.len(), 1);
        assert!(inventory.has_library("tags"));
    }
}

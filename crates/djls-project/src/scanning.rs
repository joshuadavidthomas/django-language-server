use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

use djls_extraction::collect_registrations_from_body;
use djls_extraction::EnvironmentInventory;
use djls_extraction::EnvironmentLibrary;
use djls_extraction::SymbolKind;

/// Scan Python environment paths to discover all template tag libraries.
///
/// Globs each `sys_path` entry for `*/templatetags/*.py`, skipping `__init__.py`
/// and `__pycache__` directories. Derives `load_name` from filename stem and
/// `app_module` from parent directory structure.
///
/// This is a library-level scan only — `tags` and `filters` are empty.
/// Use [`scan_environment_with_symbols`] for symbol-level extraction.
#[must_use]
pub fn scan_environment(sys_paths: &[PathBuf]) -> EnvironmentInventory {
    let mut libraries: BTreeMap<String, Vec<EnvironmentLibrary>> = BTreeMap::new();

    for sys_path in sys_paths {
        if !sys_path.is_dir() {
            continue;
        }
        scan_sys_path_entry(sys_path, false, &mut libraries);
    }

    EnvironmentInventory::new(libraries)
}

/// Scan Python environment paths and extract symbol-level information.
///
/// Like [`scan_environment`], but also parses each `templatetags/*.py` file
/// with Ruff to extract tag and filter registration names. If a file fails
/// to parse, the library is still included with empty `tags`/`filters`.
#[must_use]
pub fn scan_environment_with_symbols(sys_paths: &[PathBuf]) -> EnvironmentInventory {
    let mut libraries: BTreeMap<String, Vec<EnvironmentLibrary>> = BTreeMap::new();

    for sys_path in sys_paths {
        if !sys_path.is_dir() {
            continue;
        }
        scan_sys_path_entry(sys_path, true, &mut libraries);
    }

    EnvironmentInventory::new(libraries)
}

fn scan_sys_path_entry(
    sys_path: &Path,
    extract_symbols: bool,
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
        scan_package_tree(&path, sys_path, extract_symbols, libraries);
    }
}

fn scan_package_tree(
    dir: &Path,
    sys_path: &Path,
    extract_symbols: bool,
    libraries: &mut BTreeMap<String, Vec<EnvironmentLibrary>>,
) {
    let templatetags_dir = dir.join("templatetags");
    if templatetags_dir.is_dir() {
        let init_file = templatetags_dir.join("__init__.py");
        if init_file.exists() {
            scan_templatetags_dir(&templatetags_dir, sys_path, extract_symbols, libraries);
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
        let init = path.join("__init__.py");
        if init.exists() {
            scan_package_tree(&path, sys_path, extract_symbols, libraries);
        }
    }
}

fn scan_templatetags_dir(
    templatetags_dir: &Path,
    sys_path: &Path,
    extract_symbols: bool,
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

        let (tags, filters) = if extract_symbols {
            extract_symbols_from_file(&abs_path)
        } else {
            (Vec::new(), Vec::new())
        };

        let lib = EnvironmentLibrary {
            load_name: load_name.clone(),
            app_module,
            module_path,
            source_path: abs_path,
            tags,
            filters,
        };

        libraries.entry(load_name).or_default().push(lib);
    }
}

fn extract_symbols_from_file(path: &Path) -> (Vec<String>, Vec<String>) {
    let Ok(source) = std::fs::read_to_string(path) else {
        return (Vec::new(), Vec::new());
    };

    let Ok(parsed) = ruff_python_parser::parse_module(&source) else {
        return (Vec::new(), Vec::new());
    };

    let module = parsed.into_syntax();
    let registrations = collect_registrations_from_body(&module.body);

    let mut tags = Vec::new();
    let mut filters = Vec::new();

    for reg in registrations {
        match reg.kind.symbol_kind() {
            SymbolKind::Tag => tags.push(reg.name),
            SymbolKind::Filter => filters.push(reg.name),
        }
    }

    tags.sort();
    tags.dedup();
    filters.sort();
    filters.dedup();

    (tags, filters)
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

    fn create_templatetags_layout(root: &Path, packages: &[(&str, &[&str])]) {
        for (package_path, tag_files) in packages {
            let parts: Vec<&str> = package_path.split('/').collect();
            let mut current = root.to_path_buf();

            for part in &parts {
                current.push(part);
                std::fs::create_dir_all(&current).unwrap();
                std::fs::write(current.join("__init__.py"), "").unwrap();
            }

            let templatetags_dir = current.join("templatetags");
            std::fs::create_dir_all(&templatetags_dir).unwrap();
            std::fs::write(templatetags_dir.join("__init__.py"), "").unwrap();

            for tag_file in *tag_files {
                std::fs::write(
                    templatetags_dir.join(format!("{tag_file}.py")),
                    "# templatetag module\n",
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

        create_templatetags_layout(root, &[("pkg_a", &["utils"]), ("pkg_b", &["utils"])]);

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

        let pkg_dir = root.join("myapp");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(pkg_dir.join("__init__.py"), "").unwrap();

        let tags_dir = pkg_dir.join("templatetags");
        std::fs::create_dir_all(&tags_dir).unwrap();
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

        let inventory = scan_environment(&[tmp1.path().to_path_buf(), tmp2.path().to_path_buf()]);

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

    fn create_templatetags_with_source(root: &Path, package_path: &str, files: &[(&str, &str)]) {
        let parts: Vec<&str> = package_path.split('/').collect();
        let mut current = root.to_path_buf();
        for part in &parts {
            current.push(part);
            std::fs::create_dir_all(&current).unwrap();
            std::fs::write(current.join("__init__.py"), "").unwrap();
        }
        let templatetags_dir = current.join("templatetags");
        std::fs::create_dir_all(&templatetags_dir).unwrap();
        std::fs::write(templatetags_dir.join("__init__.py"), "").unwrap();
        for (name, source) in files {
            std::fs::write(templatetags_dir.join(format!("{name}.py")), source).unwrap();
        }
    }

    #[test]
    fn scan_with_symbols_extracts_registrations() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        create_templatetags_with_source(
            root,
            "myapp",
            &[(
                "custom",
                r#"
from django import template
register = template.Library()

@register.simple_tag
def hello():
    return "Hello!"

@register.filter
def lower(value):
    return value.lower()

@register.filter
def upper(value):
    return value.upper()
"#,
            )],
        );

        let inventory = scan_environment_with_symbols(&[root.to_path_buf()]);
        let libs = inventory.libraries_for_name("custom");
        assert_eq!(libs.len(), 1);
        assert_eq!(libs[0].tags, vec!["hello"]);
        assert_eq!(libs[0].filters, vec!["lower", "upper"]);
    }

    #[test]
    fn scan_with_symbols_parse_failure_still_discovers_library() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        create_templatetags_with_source(root, "myapp", &[("broken", "def {invalid python syntax")]);

        let inventory = scan_environment_with_symbols(&[root.to_path_buf()]);
        assert!(inventory.has_library("broken"));
        let libs = inventory.libraries_for_name("broken");
        assert_eq!(libs.len(), 1);
        assert!(libs[0].tags.is_empty());
        assert!(libs[0].filters.is_empty());
    }

    #[test]
    fn scan_with_symbols_reverse_lookup_tags() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        create_templatetags_with_source(
            root,
            "django/contrib/humanize",
            &[(
                "humanize",
                r"
from django import template
register = template.Library()

@register.filter
def intcomma(value):
    return str(value)

@register.filter
def naturaltime(value):
    return str(value)

@register.simple_tag
def show_metric(name):
    return name
",
            )],
        );

        let inventory = scan_environment_with_symbols(&[root.to_path_buf()]);
        let tags_map = inventory.tags_by_name();
        let filters_map = inventory.filters_by_name();

        assert!(tags_map.contains_key("show_metric"));
        let tag_syms = &tags_map["show_metric"];
        assert_eq!(tag_syms.len(), 1);
        assert_eq!(tag_syms[0].library_load_name, "humanize");
        assert_eq!(tag_syms[0].app_module, "django.contrib.humanize");

        assert!(filters_map.contains_key("intcomma"));
        assert!(filters_map.contains_key("naturaltime"));
        let filter_syms = &filters_map["intcomma"];
        assert_eq!(filter_syms.len(), 1);
        assert_eq!(filter_syms[0].library_load_name, "humanize");
    }

    #[test]
    fn scan_with_symbols_reverse_lookup_collision() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        let tag_source = r#"
from django import template
register = template.Library()

@register.simple_tag
def render_widget():
    return ""
"#;

        create_templatetags_with_source(root, "pkg_a", &[("widgets", tag_source)]);
        create_templatetags_with_source(root, "pkg_b", &[("widgets", tag_source)]);

        let inventory = scan_environment_with_symbols(&[root.to_path_buf()]);
        let tags_map = inventory.tags_by_name();

        let syms = &tags_map["render_widget"];
        assert_eq!(syms.len(), 2);
        let apps: Vec<&str> = syms.iter().map(|s| s.app_module.as_str()).collect();
        assert!(apps.contains(&"pkg_a"));
        assert!(apps.contains(&"pkg_b"));
    }

    #[test]
    fn scan_without_symbols_has_empty_tags_filters() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        create_templatetags_with_source(
            root,
            "myapp",
            &[(
                "custom",
                r#"
from django import template
register = template.Library()

@register.simple_tag
def hello():
    return "Hello!"
"#,
            )],
        );

        let inventory = scan_environment(&[root.to_path_buf()]);
        let libs = inventory.libraries_for_name("custom");
        assert_eq!(libs.len(), 1);
        assert!(libs[0].tags.is_empty());
        assert!(libs[0].filters.is_empty());
    }

    #[test]
    fn scan_with_symbols_no_registrations() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        create_templatetags_with_source(root, "myapp", &[("utils", "def helper():\n    pass\n")]);

        let inventory = scan_environment_with_symbols(&[root.to_path_buf()]);
        assert!(inventory.has_library("utils"));
        let libs = inventory.libraries_for_name("utils");
        assert!(libs[0].tags.is_empty());
        assert!(libs[0].filters.is_empty());
    }
}

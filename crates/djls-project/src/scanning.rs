use std::collections::BTreeMap;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_python::collect_registrations_from_body;
use djls_python::SymbolKind;

use crate::DiscoveredTemplateLibraries;
use crate::DiscoveredTemplateLibrary;

/// Scan Python environment paths to discover all template tag libraries.
///
/// Globs each `sys_path` entry for `*/templatetags/*.py`, skipping `__init__.py`
/// and `__pycache__` directories. Derives `load_name` from filename stem and
/// `app_module` from parent directory structure.
///
/// This is a library-level scan only — `tags` and `filters` are empty.
/// Use [`scan_environment_with_symbols`] for symbol-level extraction.
#[must_use]
pub fn scan_environment(sys_paths: &[Utf8PathBuf]) -> DiscoveredTemplateLibraries {
    let mut libraries: BTreeMap<String, Vec<DiscoveredTemplateLibrary>> = BTreeMap::new();

    for sys_path in sys_paths {
        if !sys_path.is_dir() {
            continue;
        }
        scan_sys_path_entry(sys_path, false, &mut libraries);
    }

    DiscoveredTemplateLibraries::new(libraries)
}

/// Scan Python environment paths and extract symbol-level information.
///
/// Like [`scan_environment`], but also parses each `templatetags/*.py` file
/// with Ruff to extract tag and filter registration names. If a file fails
/// to parse, the library is still included with empty `tags`/`filters`.
#[must_use]
pub fn scan_environment_with_symbols(sys_paths: &[Utf8PathBuf]) -> DiscoveredTemplateLibraries {
    let mut libraries: BTreeMap<String, Vec<DiscoveredTemplateLibrary>> = BTreeMap::new();

    for sys_path in sys_paths {
        if !sys_path.is_dir() {
            continue;
        }
        scan_sys_path_entry(sys_path, true, &mut libraries);
    }

    DiscoveredTemplateLibraries::new(libraries)
}

fn scan_sys_path_entry(
    sys_path: &Utf8Path,
    extract_symbols: bool,
    libraries: &mut BTreeMap<String, Vec<DiscoveredTemplateLibrary>>,
) {
    let Ok(top_entries) = std::fs::read_dir(sys_path.as_std_path()) else {
        return;
    };

    for entry in top_entries.flatten() {
        let Ok(path) = Utf8PathBuf::from_path_buf(entry.path()) else {
            continue;
        };

        if !path.is_dir() {
            continue;
        }

        scan_package_tree(&path, sys_path, extract_symbols, libraries);
    }
}

fn scan_package_tree(
    dir: &Utf8Path,
    sys_path: &Utf8Path,
    extract_symbols: bool,
    libraries: &mut BTreeMap<String, Vec<DiscoveredTemplateLibrary>>,
) {
    let templatetags_dir = dir.join("templatetags");
    if templatetags_dir.is_dir() {
        let init_file = templatetags_dir.join("__init__.py");
        if init_file.exists() {
            scan_templatetags_dir(&templatetags_dir, sys_path, extract_symbols, libraries);
        }
    }

    let Ok(entries) = std::fs::read_dir(dir.as_std_path()) else {
        return;
    };

    for entry in entries.flatten() {
        let Ok(path) = Utf8PathBuf::from_path_buf(entry.path()) else {
            continue;
        };

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
    templatetags_dir: &Utf8Path,
    sys_path: &Utf8Path,
    extract_symbols: bool,
    libraries: &mut BTreeMap<String, Vec<DiscoveredTemplateLibrary>>,
) {
    let Ok(entries) = std::fs::read_dir(templatetags_dir.as_std_path()) else {
        return;
    };

    for entry in entries.flatten() {
        let Ok(path) = Utf8PathBuf::from_path_buf(entry.path()) else {
            continue;
        };

        if !path.is_file() {
            continue;
        }

        let Some(ext) = path.extension() else {
            continue;
        };
        if ext != "py" {
            continue;
        }

        let Some(stem) = path.file_stem() else {
            continue;
        };
        if stem == "__init__" {
            continue;
        }

        let load_name = stem.to_string();

        let Some(parent) = templatetags_dir.parent() else {
            continue;
        };
        let Ok(rel_path) = parent.strip_prefix(sys_path) else {
            continue;
        };
        let app_module = path_to_dotted(rel_path);

        let Ok(full_rel) = path.strip_prefix(sys_path) else {
            continue;
        };
        let module_path = path_to_dotted_strip_py(full_rel);

        let abs_path = if path.is_absolute() {
            path.clone()
        } else {
            match std::env::current_dir() {
                Ok(cwd) => {
                    Utf8PathBuf::from_path_buf(cwd.join(path.as_std_path())).unwrap_or(path.clone())
                }
                Err(_) => path.clone(),
            }
        };

        let (tags, filters) = if extract_symbols {
            extract_symbols_from_file(&abs_path)
        } else {
            (Vec::new(), Vec::new())
        };

        let lib = DiscoveredTemplateLibrary {
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

fn extract_symbols_from_file(path: &Utf8Path) -> (Vec<String>, Vec<String>) {
    let Ok(source) = std::fs::read_to_string(path.as_std_path()) else {
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

fn path_to_dotted(rel_path: &Utf8Path) -> String {
    rel_path
        .components()
        .map(|c| c.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

fn path_to_dotted_strip_py(rel_path: &Utf8Path) -> String {
    let dotted = path_to_dotted(rel_path);
    dotted.strip_suffix(".py").unwrap_or(&dotted).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf8_tmpdir() -> (tempfile::TempDir, Utf8PathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        (tmp, root)
    }

    fn create_templatetags_layout(root: &Utf8Path, packages: &[(&str, &[&str])]) {
        for (package_path, tag_files) in packages {
            let parts: Vec<&str> = package_path.split('/').collect();
            let mut current = root.to_path_buf();

            for part in &parts {
                current.push(part);
                std::fs::create_dir_all(current.as_std_path()).unwrap();
                std::fs::write(current.join("__init__.py").as_std_path(), "").unwrap();
            }

            let templatetags_dir = current.join("templatetags");
            std::fs::create_dir_all(templatetags_dir.as_std_path()).unwrap();
            std::fs::write(templatetags_dir.join("__init__.py").as_std_path(), "").unwrap();

            for tag_file in *tag_files {
                std::fs::write(
                    templatetags_dir
                        .join(format!("{tag_file}.py"))
                        .as_std_path(),
                    "# templatetag module\n",
                )
                .unwrap();
            }
        }
    }

    #[test]
    fn scan_discovers_libraries() {
        let (_tmp, root) = utf8_tmpdir();

        create_templatetags_layout(
            &root,
            &[
                ("django/contrib/humanize", &["humanize"]),
                ("django/contrib/admin", &["admin_list", "admin_modify"]),
            ],
        );

        let inventory = scan_environment(std::slice::from_ref(&root));

        assert!(inventory.has_library("humanize"));
        assert!(inventory.has_library("admin_list"));
        assert!(inventory.has_library("admin_modify"));
        assert!(!inventory.has_library("__init__"));
    }

    #[test]
    fn scan_derives_correct_app_module() {
        let (_tmp, root) = utf8_tmpdir();

        create_templatetags_layout(&root, &[("django/contrib/humanize", &["humanize"])]);

        let inventory = scan_environment(std::slice::from_ref(&root));
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
        let (_tmp, root) = utf8_tmpdir();

        create_templatetags_layout(&root, &[("pkg_a", &["utils"]), ("pkg_b", &["utils"])]);

        let inventory = scan_environment(std::slice::from_ref(&root));
        let libs = inventory.libraries_for_name("utils");
        assert_eq!(libs.len(), 2);

        let app_modules: Vec<&str> = libs.iter().map(|l| l.app_module.as_str()).collect();
        assert!(app_modules.contains(&"pkg_a"));
        assert!(app_modules.contains(&"pkg_b"));
    }

    #[test]
    fn scan_skips_init_files() {
        let (_tmp, root) = utf8_tmpdir();

        create_templatetags_layout(&root, &[("myapp", &["custom"])]);

        let inventory = scan_environment(std::slice::from_ref(&root));
        assert!(!inventory.has_library("__init__"));
        assert!(inventory.has_library("custom"));
    }

    #[test]
    fn scan_requires_templatetags_init() {
        let (_tmp, root) = utf8_tmpdir();

        let pkg_dir = root.join("myapp");
        std::fs::create_dir_all(pkg_dir.as_std_path()).unwrap();
        std::fs::write(pkg_dir.join("__init__.py").as_std_path(), "").unwrap();

        let tags_dir = pkg_dir.join("templatetags");
        std::fs::create_dir_all(tags_dir.as_std_path()).unwrap();
        std::fs::write(tags_dir.join("custom.py").as_std_path(), "# tag module").unwrap();

        let inventory = scan_environment(std::slice::from_ref(&root));
        assert!(!inventory.has_library("custom"));
    }

    #[test]
    fn scan_empty_directory() {
        let (_tmp, root) = utf8_tmpdir();

        let inventory = scan_environment(&[root]);
        assert!(inventory.is_empty());
    }

    #[test]
    fn scan_nonexistent_path() {
        let inventory = scan_environment(&[Utf8PathBuf::from("/nonexistent/path/12345")]);
        assert!(inventory.is_empty());
    }

    #[test]
    fn scan_multiple_sys_paths() {
        let (_tmp1, root1) = utf8_tmpdir();
        let (_tmp2, root2) = utf8_tmpdir();

        create_templatetags_layout(&root1, &[("pkg1", &["tags1"])]);
        create_templatetags_layout(&root2, &[("pkg2", &["tags2"])]);

        let inventory = scan_environment(&[root1, root2]);

        assert!(inventory.has_library("tags1"));
        assert!(inventory.has_library("tags2"));
        assert_eq!(inventory.len(), 2);
    }

    #[test]
    fn libraries_for_unknown_name_returns_empty() {
        let inventory = DiscoveredTemplateLibraries::default();
        assert!(inventory.libraries_for_name("nonexistent").is_empty());
    }

    #[test]
    fn scan_skips_non_py_files() {
        let (_tmp, root) = utf8_tmpdir();

        let pkg_dir = root.join("myapp");
        std::fs::create_dir_all(pkg_dir.as_std_path()).unwrap();
        std::fs::write(pkg_dir.join("__init__.py").as_std_path(), "").unwrap();

        let tags_dir = pkg_dir.join("templatetags");
        std::fs::create_dir_all(tags_dir.as_std_path()).unwrap();
        std::fs::write(tags_dir.join("__init__.py").as_std_path(), "").unwrap();
        std::fs::write(tags_dir.join("tags.py").as_std_path(), "# tag").unwrap();
        std::fs::write(tags_dir.join("readme.txt").as_std_path(), "# readme").unwrap();
        std::fs::write(tags_dir.join("data.json").as_std_path(), "{}").unwrap();

        let inventory = scan_environment(std::slice::from_ref(&root));
        assert_eq!(inventory.len(), 1);
        assert!(inventory.has_library("tags"));
    }

    fn create_templatetags_with_source(
        root: &Utf8Path,
        package_path: &str,
        files: &[(&str, &str)],
    ) {
        let parts: Vec<&str> = package_path.split('/').collect();
        let mut current = root.to_path_buf();
        for part in &parts {
            current.push(part);
            std::fs::create_dir_all(current.as_std_path()).unwrap();
            std::fs::write(current.join("__init__.py").as_std_path(), "").unwrap();
        }
        let templatetags_dir = current.join("templatetags");
        std::fs::create_dir_all(templatetags_dir.as_std_path()).unwrap();
        std::fs::write(templatetags_dir.join("__init__.py").as_std_path(), "").unwrap();
        for (name, source) in files {
            std::fs::write(
                templatetags_dir.join(format!("{name}.py")).as_std_path(),
                source,
            )
            .unwrap();
        }
    }

    #[test]
    fn scan_with_symbols_extracts_registrations() {
        let (_tmp, root) = utf8_tmpdir();

        create_templatetags_with_source(
            &root,
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

        let inventory = scan_environment_with_symbols(std::slice::from_ref(&root));
        let libs = inventory.libraries_for_name("custom");
        assert_eq!(libs.len(), 1);
        assert_eq!(libs[0].tags, vec!["hello"]);
        assert_eq!(libs[0].filters, vec!["lower", "upper"]);
    }

    #[test]
    fn scan_with_symbols_parse_failure_still_discovers_library() {
        let (_tmp, root) = utf8_tmpdir();

        create_templatetags_with_source(
            &root,
            "myapp",
            &[("broken", "def {invalid python syntax")],
        );

        let inventory = scan_environment_with_symbols(std::slice::from_ref(&root));
        assert!(inventory.has_library("broken"));
        let libs = inventory.libraries_for_name("broken");
        assert_eq!(libs.len(), 1);
        assert!(libs[0].tags.is_empty());
        assert!(libs[0].filters.is_empty());
    }

    #[test]
    fn scan_with_symbols_reverse_lookup_tags() {
        let (_tmp, root) = utf8_tmpdir();

        create_templatetags_with_source(
            &root,
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

        let inventory = scan_environment_with_symbols(std::slice::from_ref(&root));
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
        let (_tmp, root) = utf8_tmpdir();

        let tag_source = r#"
from django import template
register = template.Library()

@register.simple_tag
def render_widget():
    return ""
"#;

        create_templatetags_with_source(&root, "pkg_a", &[("widgets", tag_source)]);
        create_templatetags_with_source(&root, "pkg_b", &[("widgets", tag_source)]);

        let inventory = scan_environment_with_symbols(std::slice::from_ref(&root));
        let tags_map = inventory.tags_by_name();

        let syms = &tags_map["render_widget"];
        assert_eq!(syms.len(), 2);
        let apps: Vec<&str> = syms.iter().map(|s| s.app_module.as_str()).collect();
        assert!(apps.contains(&"pkg_a"));
        assert!(apps.contains(&"pkg_b"));
    }

    #[test]
    fn scan_without_symbols_has_empty_tags_filters() {
        let (_tmp, root) = utf8_tmpdir();

        create_templatetags_with_source(
            &root,
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

        let inventory = scan_environment(std::slice::from_ref(&root));
        let libs = inventory.libraries_for_name("custom");
        assert_eq!(libs.len(), 1);
        assert!(libs[0].tags.is_empty());
        assert!(libs[0].filters.is_empty());
    }

    #[test]
    fn scan_with_symbols_no_registrations() {
        let (_tmp, root) = utf8_tmpdir();

        create_templatetags_with_source(&root, "myapp", &[("utils", "def helper():\n    pass\n")]);

        let inventory = scan_environment_with_symbols(std::slice::from_ref(&root));
        assert!(inventory.has_library("utils"));
        let libs = inventory.libraries_for_name("utils");
        assert!(libs[0].tags.is_empty());
        assert!(libs[0].filters.is_empty());
    }
}

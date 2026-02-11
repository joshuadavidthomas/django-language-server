use std::collections::BTreeMap;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_python::collect_registrations_from_body;
use djls_python::SymbolKind;
use rustc_hash::FxHashSet;

use crate::scanned_libraries::ScannedTemplateLibraries;
use crate::scanned_libraries::ScannedTemplateLibrary;
use crate::scanned_libraries::ScannedTemplateSymbol;
use crate::template_libraries::TemplateSymbolKind;
use crate::template_names::LibraryName;
use crate::template_names::PyModuleName;
use crate::template_names::TemplateSymbolName;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SymbolExtraction {
    None,
    WithSymbols,
}

/// Scan Python environment paths to discover all template tag libraries.
///
/// Globs each `sys_path` entry for `*/templatetags/*.py`, skipping `__init__.py`
/// and `__pycache__` directories. Derives the [`LibraryName`] from the file stem and
/// the app module from the parent package structure.
///
/// This is a library-level scan only — symbols are empty.
/// Use [`scan_template_libraries_with_symbols`] for symbol-level extraction.
#[must_use]
pub fn scan_template_libraries(sys_paths: &[Utf8PathBuf]) -> ScannedTemplateLibraries {
    scan_template_libraries_impl(sys_paths, SymbolExtraction::None)
}

/// Scan Python environment paths and extract symbol-level information.
///
/// Like [`scan_template_libraries`], but also parses each `templatetags/*.py` file
/// with Ruff to extract tag and filter registration names. If a file fails
/// to parse, the library is still included with an empty symbol list.
#[must_use]
pub fn scan_template_libraries_with_symbols(sys_paths: &[Utf8PathBuf]) -> ScannedTemplateLibraries {
    scan_template_libraries_impl(sys_paths, SymbolExtraction::WithSymbols)
}

fn scan_template_libraries_impl(
    sys_paths: &[Utf8PathBuf],
    extraction: SymbolExtraction,
) -> ScannedTemplateLibraries {
    let mut libraries: BTreeMap<LibraryName, Vec<ScannedTemplateLibrary>> = BTreeMap::new();
    let mut visited: FxHashSet<Utf8PathBuf> = FxHashSet::default();

    for sys_path in sys_paths {
        if !sys_path.is_dir() {
            continue;
        }
        scan_sys_path_entry(sys_path, extraction, &mut visited, &mut libraries);
    }

    ScannedTemplateLibraries::new(libraries)
}

fn scan_sys_path_entry(
    sys_path: &Utf8Path,
    extraction: SymbolExtraction,
    visited: &mut FxHashSet<Utf8PathBuf>,
    libraries: &mut BTreeMap<LibraryName, Vec<ScannedTemplateLibrary>>,
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

        scan_package_tree(&path, sys_path, extraction, visited, libraries);
    }
}

fn scan_package_tree(
    dir: &Utf8Path,
    sys_path: &Utf8Path,
    extraction: SymbolExtraction,
    visited: &mut FxHashSet<Utf8PathBuf>,
    libraries: &mut BTreeMap<LibraryName, Vec<ScannedTemplateLibrary>>,
) {
    let key = std::fs::canonicalize(dir.as_std_path())
        .ok()
        .and_then(|p| Utf8PathBuf::from_path_buf(p).ok())
        .unwrap_or_else(|| dir.to_owned());

    if !visited.insert(key) {
        return;
    }

    let templatetags_dir = dir.join("templatetags");
    if templatetags_dir.is_dir() {
        let init_file = templatetags_dir.join("__init__.py");
        if init_file.exists() {
            scan_templatetags_dir(&templatetags_dir, sys_path, extraction, libraries);
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
            scan_package_tree(&path, sys_path, extraction, visited, libraries);
        }
    }
}

fn scan_templatetags_dir(
    templatetags_dir: &Utf8Path,
    sys_path: &Utf8Path,
    extraction: SymbolExtraction,
    libraries: &mut BTreeMap<LibraryName, Vec<ScannedTemplateLibrary>>,
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

        if path.extension() != Some("py") {
            continue;
        }

        let Some(stem) = path.file_stem() else {
            continue;
        };
        if stem == "__init__" {
            continue;
        }

        let Some(library_name) = LibraryName::new(stem) else {
            continue;
        };

        let Some(parent) = templatetags_dir.parent() else {
            continue;
        };
        let Ok(rel_path) = parent.strip_prefix(sys_path) else {
            continue;
        };

        let Some(app_module) = PyModuleName::new(&path_to_dotted(rel_path)) else {
            continue;
        };

        let Ok(full_rel) = path.strip_prefix(sys_path) else {
            continue;
        };

        let Some(module) = PyModuleName::new(&path_to_dotted_strip_py(full_rel)) else {
            continue;
        };

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

        let symbols = match extraction {
            SymbolExtraction::WithSymbols => extract_symbols_from_file(&abs_path),
            SymbolExtraction::None => Vec::new(),
        };

        let library = ScannedTemplateLibrary {
            name: library_name.clone(),
            app_module,
            module,
            source_path: abs_path,
            symbols,
        };

        libraries.entry(library_name).or_default().push(library);
    }
}

fn extract_symbols_from_file(path: &Utf8Path) -> Vec<ScannedTemplateSymbol> {
    let Ok(source) = std::fs::read_to_string(path.as_std_path()) else {
        return Vec::new();
    };

    let Ok(parsed) = ruff_python_parser::parse_module(&source) else {
        return Vec::new();
    };

    let module = parsed.into_syntax();
    let registrations = collect_registrations_from_body(&module.body);

    let mut symbols = Vec::new();

    for reg in registrations {
        let kind = match reg.kind.symbol_kind() {
            SymbolKind::Tag => TemplateSymbolKind::Tag,
            SymbolKind::Filter => TemplateSymbolKind::Filter,
        };

        let Some(name) = TemplateSymbolName::new(&reg.name) else {
            continue;
        };

        symbols.push(ScannedTemplateSymbol { kind, name });
    }

    symbols.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.name.cmp(&b.name)));
    symbols.dedup_by(|a, b| a.kind == b.kind && a.name == b.name);

    symbols
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

    fn create_templatetags_with_source(
        root: &Utf8Path,
        package_path: &str,
        tag_files: &[(&str, &str)],
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

        for (tag_file, source) in tag_files {
            std::fs::write(
                templatetags_dir
                    .join(format!("{tag_file}.py"))
                    .as_std_path(),
                source,
            )
            .unwrap();
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

        let inventory = scan_template_libraries(std::slice::from_ref(&root));

        assert!(inventory.has_library(&LibraryName::new("humanize").unwrap()));
        assert!(inventory.has_library(&LibraryName::new("admin_list").unwrap()));
        assert!(inventory.has_library(&LibraryName::new("admin_modify").unwrap()));
        assert!(!inventory.has_library(&LibraryName::new("nonexistent_library").unwrap()));
    }

    #[test]
    fn scan_derives_correct_app_module() {
        let (_tmp, root) = utf8_tmpdir();

        create_templatetags_layout(&root, &[("django/contrib/humanize", &["humanize"])]);

        let inventory = scan_template_libraries(std::slice::from_ref(&root));
        let libs = inventory.libraries_for_name(&LibraryName::new("humanize").unwrap());
        assert_eq!(libs.len(), 1);

        assert_eq!(libs[0].app_module.as_str(), "django.contrib.humanize");
    }

    #[test]
    fn scan_derives_correct_module_path() {
        let (_tmp, root) = utf8_tmpdir();

        create_templatetags_layout(&root, &[("django/contrib/humanize", &["humanize"])]);

        let inventory = scan_template_libraries(std::slice::from_ref(&root));
        let libs = inventory.libraries_for_name(&LibraryName::new("humanize").unwrap());
        assert_eq!(libs.len(), 1);

        assert_eq!(
            libs[0].module.as_str(),
            "django.contrib.humanize.templatetags.humanize"
        );
    }

    #[test]
    fn scan_skips_non_python_files() {
        let (_tmp, root) = utf8_tmpdir();
        create_templatetags_layout(&root, &[("myapp", &["custom"])]);

        let templatetags_dir = root.join("myapp/templatetags");
        std::fs::write(templatetags_dir.join("not_python.txt").as_std_path(), "").unwrap();

        let inventory = scan_template_libraries(std::slice::from_ref(&root));
        assert!(inventory.has_library(&LibraryName::new("custom").unwrap()));
        assert!(!inventory.has_library(&LibraryName::new("not_python").unwrap()));
    }

    #[test]
    fn scan_handles_invalid_sys_paths() {
        let inventory = scan_template_libraries(&[Utf8PathBuf::from("/nonexistent/path/12345")]);
        assert!(inventory.is_empty());
    }

    #[test]
    fn scan_with_symbols_includes_libraries_even_if_parse_fails() {
        let (_tmp, root) = utf8_tmpdir();

        create_templatetags_with_source(
            &root,
            "django/contrib/humanize",
            &[("broken", "def this is not python\n")],
        );

        let inventory = scan_template_libraries_with_symbols(std::slice::from_ref(&root));
        assert!(inventory.has_library(&LibraryName::new("broken").unwrap()));
        let libs = inventory.libraries_for_name(&LibraryName::new("broken").unwrap());
        assert_eq!(libs.len(), 1);
        assert!(libs[0].symbols.is_empty());
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

        let inventory = scan_template_libraries_with_symbols(std::slice::from_ref(&root));
        let tags_map = inventory.tags_by_name();
        let filters_map = inventory.filters_by_name();

        let show_metric = TemplateSymbolName::new("show_metric").unwrap();
        assert!(tags_map.contains_key(&show_metric));
        let tag_syms = &tags_map[&show_metric];
        assert_eq!(tag_syms.len(), 1);
        assert_eq!(tag_syms[0].library_name.as_str(), "humanize");
        assert_eq!(tag_syms[0].app_module.as_str(), "django.contrib.humanize");

        let intcomma = TemplateSymbolName::new("intcomma").unwrap();
        let naturaltime = TemplateSymbolName::new("naturaltime").unwrap();
        assert!(filters_map.contains_key(&intcomma));
        assert!(filters_map.contains_key(&naturaltime));
        let filter_syms = &filters_map[&intcomma];
        assert_eq!(filter_syms.len(), 1);
        assert_eq!(filter_syms[0].library_name.as_str(), "humanize");
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

        let inventory = scan_template_libraries_with_symbols(std::slice::from_ref(&root));
        let tags_map = inventory.tags_by_name();

        let render_widget = TemplateSymbolName::new("render_widget").unwrap();
        let syms = &tags_map[&render_widget];
        assert_eq!(syms.len(), 2);
        let apps: Vec<&str> = syms.iter().map(|s| s.app_module.as_str()).collect();
        assert!(apps.contains(&"pkg_a"));
        assert!(apps.contains(&"pkg_b"));
    }

    #[test]
    fn scan_without_symbols_has_empty_symbol_lists() {
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

        let inventory = scan_template_libraries(std::slice::from_ref(&root));
        let libs = inventory.libraries_for_name(&LibraryName::new("custom").unwrap());
        assert_eq!(libs.len(), 1);
        assert!(libs[0].symbols.is_empty());
    }

    #[test]
    fn scan_with_symbols_no_registrations() {
        let (_tmp, root) = utf8_tmpdir();

        create_templatetags_with_source(&root, "myapp", &[("utils", "def helper():\n    pass\n")]);

        let inventory = scan_template_libraries_with_symbols(std::slice::from_ref(&root));
        assert!(inventory.has_library(&LibraryName::new("utils").unwrap()));
        let libs = inventory.libraries_for_name(&LibraryName::new("utils").unwrap());
        assert!(libs[0].symbols.is_empty());
    }
}

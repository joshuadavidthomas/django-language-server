use std::collections::BTreeMap;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_python::collect_registrations_from_source;
use djls_python::SymbolKind;
use rustc_hash::FxHashSet;

use crate::names::LibraryName;
use crate::names::PyModuleName;
use crate::names::TemplateSymbolName;
use crate::symbols::LibraryOrigin;
use crate::symbols::SymbolDefinition;
use crate::symbols::TemplateLibrary;
use crate::symbols::TemplateSymbol;
use crate::symbols::TemplateSymbolKind;

/// Discover template libraries available in Python search paths.
///
/// Scans package trees under each `sys_path` entry, finds
/// `*/templatetags/*.py` modules, derives load names and module origins,
/// and extracts tag/filter registrations from each module.
#[must_use]
pub fn discover_template_libraries(sys_paths: &[Utf8PathBuf]) -> Vec<TemplateLibrary> {
    let mut libraries: BTreeMap<LibraryName, Vec<TemplateLibrary>> = BTreeMap::new();
    let mut visited: FxHashSet<Utf8PathBuf> = FxHashSet::default();

    for sys_path in sys_paths {
        if !sys_path.is_dir() {
            continue;
        }

        let Ok(top_entries) = std::fs::read_dir(sys_path.as_std_path()) else {
            continue;
        };

        for entry in top_entries.flatten() {
            let Ok(path) = Utf8PathBuf::from_path_buf(entry.path()) else {
                continue;
            };

            if !path.is_dir() {
                continue;
            }

            discover_package_tree(&path, sys_path, &mut visited, &mut libraries);
        }
    }

    libraries.into_values().flatten().collect()
}

fn discover_package_tree(
    dir: &Utf8Path,
    sys_path: &Utf8Path,
    visited: &mut FxHashSet<Utf8PathBuf>,
    libraries: &mut BTreeMap<LibraryName, Vec<TemplateLibrary>>,
) {
    let key = std::fs::canonicalize(dir.as_std_path())
        .ok()
        .and_then(|p| Utf8PathBuf::from_path_buf(p).ok())
        .unwrap_or_else(|| dir.to_owned());

    if !visited.insert(key) {
        return;
    }

    let templatetags_dir = dir.join("templatetags");
    if templatetags_dir.is_dir() && templatetags_dir.join("__init__.py").exists() {
        let Ok(entries) = std::fs::read_dir(templatetags_dir.as_std_path()) else {
            return;
        };

        for entry in entries.flatten() {
            let Ok(path) = Utf8PathBuf::from_path_buf(entry.path()) else {
                continue;
            };

            if !path.is_file() || path.extension() != Some("py") {
                continue;
            }

            let Some(stem) = path.file_stem() else {
                continue;
            };
            if stem == "__init__" {
                continue;
            }

            let Ok(library_name) = LibraryName::parse(stem) else {
                continue;
            };

            let Some(package_dir) = templatetags_dir.parent() else {
                continue;
            };
            let Ok(package_rel_path) = package_dir.strip_prefix(sys_path) else {
                continue;
            };
            let Ok(app_module) = PyModuleName::from_relative_package(package_rel_path) else {
                continue;
            };

            let Ok(module_rel_path) = path.strip_prefix(sys_path) else {
                continue;
            };
            let Ok(module) = PyModuleName::from_relative_python_module(module_rel_path) else {
                continue;
            };

            let absolute_path = absolute_path(&path);
            let symbols = extract_symbols_from_module_file(&absolute_path);

            let origin = LibraryOrigin {
                app: app_module,
                module,
                path: absolute_path,
            };

            let mut library = TemplateLibrary::new_discovered(library_name.clone(), origin);
            library.symbols = symbols;
            libraries.entry(library_name).or_default().push(library);
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

        if path.join("__init__.py").exists() {
            discover_package_tree(&path, sys_path, visited, libraries);
        }
    }
}

fn absolute_path(path: &Utf8Path) -> Utf8PathBuf {
    if path.is_absolute() {
        return path.to_owned();
    }

    let Ok(cwd) = std::env::current_dir() else {
        return path.to_owned();
    };

    Utf8PathBuf::from_path_buf(cwd.join(path.as_std_path())).unwrap_or_else(|_| path.to_owned())
}

fn extract_symbols_from_module_file(path: &Utf8Path) -> Vec<TemplateSymbol> {
    let Ok(source) = std::fs::read_to_string(path.as_std_path()) else {
        return Vec::new();
    };

    let mut symbols = collect_registrations_from_source(&source)
        .into_iter()
        .filter_map(|registration| {
            let kind = match registration.kind.symbol_kind() {
                SymbolKind::Tag => TemplateSymbolKind::Tag,
                SymbolKind::Filter => TemplateSymbolKind::Filter,
            };

            let name = TemplateSymbolName::parse(&registration.name).ok()?;

            Some(TemplateSymbol {
                kind,
                name,
                definition: SymbolDefinition::LibraryFile(path.to_path_buf()),
                doc: None,
            })
        })
        .collect::<Vec<_>>();

    symbols.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.name.cmp(&b.name)));
    symbols.dedup_by(|a, b| a.kind == b.kind && a.name == b.name);
    symbols
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::symbols::LibraryStatus;

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
    fn finds_template_libraries() {
        let (_tmp, root) = utf8_tmpdir();

        create_templatetags_layout(
            &root,
            &[
                ("django/contrib/humanize", &["humanize"]),
                ("django/contrib/admin", &["admin_list", "admin_modify"]),
            ],
        );

        let libs = discover_template_libraries(std::slice::from_ref(&root));
        let names: Vec<String> = libs.iter().map(|l| l.name.as_str().to_string()).collect();

        assert!(names.contains(&"humanize".to_string()));
        assert!(names.contains(&"admin_list".to_string()));
        assert!(names.contains(&"admin_modify".to_string()));
    }

    #[test]
    fn derives_app_module_from_package_path() {
        let (_tmp, root) = utf8_tmpdir();

        create_templatetags_layout(&root, &[("django/contrib/humanize", &["humanize"])]);

        let libs = discover_template_libraries(std::slice::from_ref(&root));
        assert_eq!(libs.len(), 1);

        let LibraryStatus::Discovered(origin) = &libs[0].status else {
            panic!()
        };
        assert_eq!(origin.app.as_str(), "django.contrib.humanize");
    }

    #[test]
    fn derives_module_path_from_library_file() {
        let (_tmp, root) = utf8_tmpdir();

        create_templatetags_layout(&root, &[("django/contrib/humanize", &["humanize"])]);

        let libs = discover_template_libraries(std::slice::from_ref(&root));
        assert_eq!(libs.len(), 1);

        let LibraryStatus::Discovered(origin) = &libs[0].status else {
            panic!()
        };
        assert_eq!(
            origin.module.as_str(),
            "django.contrib.humanize.templatetags.humanize"
        );
    }

    #[test]
    fn ignores_non_python_files() {
        let (_tmp, root) = utf8_tmpdir();
        create_templatetags_layout(&root, &[("myapp", &["custom"])]);

        let templatetags_dir = root.join("myapp/templatetags");
        std::fs::write(templatetags_dir.join("not_python.txt").as_std_path(), "").unwrap();

        let libs = discover_template_libraries(std::slice::from_ref(&root));

        assert_eq!(libs.len(), 1);
        assert_eq!(libs[0].name.as_str(), "custom");
    }

    #[test]
    fn returns_empty_for_invalid_sys_paths() {
        let libs = discover_template_libraries(&[Utf8PathBuf::from("/nonexistent/path/12345")]);
        assert!(libs.is_empty());
    }

    #[test]
    fn keeps_library_when_symbol_parse_fails() {
        let (_tmp, root) = utf8_tmpdir();

        create_templatetags_with_source(
            &root,
            "django/contrib/humanize",
            &[("broken", "def this is not python\n")],
        );

        let libs = discover_template_libraries(std::slice::from_ref(&root));
        assert_eq!(libs.len(), 1);
        assert_eq!(libs[0].name.as_str(), "broken");
        assert!(libs[0].symbols.is_empty());
    }

    #[test]
    fn extracts_tag_and_filter_symbols() {
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

@register.simple_tag
def show_metric(name):
    return name
",
            )],
        );

        let libs = discover_template_libraries(std::slice::from_ref(&root));
        assert_eq!(libs.len(), 1);
        let lib = &libs[0];

        assert_eq!(lib.symbols.len(), 2);
        assert!(lib
            .symbols
            .iter()
            .any(|s| s.name() == "intcomma" && s.kind == TemplateSymbolKind::Filter));
        assert!(lib
            .symbols
            .iter()
            .any(|s| s.name() == "show_metric" && s.kind == TemplateSymbolKind::Tag));
    }
}

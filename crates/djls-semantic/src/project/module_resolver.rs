//! Python module resolver for project facts.
//!
//! This module resolves dotted Python module names through module search paths.
//! It models the import-resolution behavior that project fact assembly needs
//! without importing project code through the runtime inspector.

#![allow(
    dead_code,
    reason = "Milestone A2 adds module resolution before project facts are assembled."
)]

use camino::Utf8Path;
use camino::Utf8PathBuf;

use crate::project::facts::Fact;
use crate::project::facts::ModuleLocation;
use crate::project::facts::ModuleResolution;
use crate::project::facts::ModuleSearchPathEntry;
use crate::project::facts::ModuleSearchPathKind;
use crate::project::facts::Reason;
use crate::project::facts::ResolvedModule;
use crate::project::names::PyModuleName;

struct ModuleCandidate {
    module: ResolvedModule,
}

#[must_use]
pub(crate) fn discover_module_search_paths(
    project_root: &Utf8Path,
    explicit_python_paths: &[Utf8PathBuf],
    site_packages_paths: &[Utf8PathBuf],
) -> Fact<Vec<ModuleSearchPathEntry>> {
    let mut search_paths = Vec::new();
    let mut reasons = Vec::new();

    if project_root.is_dir() {
        push_module_search_path(
            &mut search_paths,
            ModuleSearchPathKind::Workspace,
            project_root.to_path_buf(),
        );
    } else {
        reasons.push(Reason::path(
            project_root,
            "project root does not exist or is not a directory",
        ));
    }

    let src_root = project_root.join("src");
    if src_root.is_dir() {
        push_module_search_path(&mut search_paths, ModuleSearchPathKind::AutoSrc, src_root);
    }

    for explicit_python_path in explicit_python_paths {
        let path = if explicit_python_path.is_absolute() {
            explicit_python_path.clone()
        } else {
            project_root.join(explicit_python_path)
        };
        if path.is_dir() {
            push_module_search_path(
                &mut search_paths,
                ModuleSearchPathKind::ExplicitPythonPath,
                path,
            );
        } else {
            reasons.push(Reason::path(
                path,
                "explicit Python module search path does not exist or is not a directory",
            ));
        }
    }

    for site_packages_path in site_packages_paths {
        if site_packages_path.is_dir() {
            push_module_search_path(
                &mut search_paths,
                ModuleSearchPathKind::SitePackages,
                site_packages_path.clone(),
            );
            collect_pth_module_search_paths(site_packages_path, &mut search_paths, &mut reasons);
        } else {
            reasons.push(Reason::path(
                site_packages_path,
                "site-packages module search path does not exist or is not a directory",
            ));
        }
    }

    if search_paths.is_empty() {
        Fact::unknown(reasons)
    } else if reasons.is_empty() {
        Fact::known(search_paths)
    } else {
        Fact::partial(search_paths, reasons)
    }
}

#[must_use]
pub(crate) fn resolve_module(
    requested: PyModuleName,
    search_paths: &[ModuleSearchPathEntry],
    project_root: &Utf8Path,
) -> ModuleResolution {
    let mut candidates = search_paths
        .iter()
        .filter_map(|root| module_candidate(&requested, root, project_root))
        .fold(Vec::new(), |mut candidates, candidate| {
            if !candidates
                .iter()
                .any(|existing: &ModuleCandidate| existing.module.file == candidate.module.file)
            {
                candidates.push(candidate);
            }
            candidates
        });

    let resolved = match candidates.len() {
        0 => Fact::unknown(vec![Reason::module(
            requested.clone(),
            format!(
                "module `{}` was not found in module search paths",
                requested.as_str()
            ),
        )]),
        1 => Fact::known(candidates.pop().unwrap().module),
        _ => Fact::ambiguous(
            candidates
                .into_iter()
                .map(|candidate| candidate.module)
                .collect(),
            vec![Reason::module(
                requested.clone(),
                "module resolves to more than one module search path",
            )],
        ),
    };

    ModuleResolution {
        requested,
        resolved,
    }
}

#[must_use]
pub(crate) fn module_name_for_file(
    file: &Utf8Path,
    search_paths: &[ModuleSearchPathEntry],
) -> Fact<PyModuleName> {
    if !file.is_file() {
        return Fact::unknown(vec![Reason::file(
            file,
            "module file does not exist or is not a file",
        )]);
    }

    if file.extension() != Some("py") {
        return Fact::unknown(vec![Reason::file(file, "module file is not a Python file")]);
    }

    let candidates = module_names_for_file(file, search_paths);
    match candidates.len() {
        0 => Fact::unknown(vec![Reason::file(
            file,
            "module file is outside configured module search paths",
        )]),
        1 => {
            let (module, _) = candidates.into_iter().next().unwrap();
            Fact::known(module)
        }
        _ => Fact::ambiguous(
            candidates.into_iter().map(|(module, _)| module).collect(),
            vec![Reason::file(
                file,
                "module file maps to more than one module name",
            )],
        ),
    }
}

#[must_use]
pub(crate) fn resolve_relative_import_module(
    current_module: &PyModuleName,
    level: usize,
    module: Option<&str>,
) -> Fact<PyModuleName> {
    if level == 0 {
        return Fact::unknown(vec![Reason::module(
            current_module.clone(),
            "relative import level must be greater than zero",
        )]);
    }

    let mut parts = current_module.as_str().split('.').collect::<Vec<_>>();
    parts.pop();

    if level > parts.len() + 1 {
        return Fact::unknown(vec![Reason::module(
            current_module.clone(),
            "relative import escapes the top-level package",
        )]);
    }

    for _ in 1..level {
        parts.pop();
    }

    if let Some(module) = module.map(str::trim).filter(|module| !module.is_empty()) {
        parts.extend(module.split('.'));
    }

    if parts.is_empty() {
        return Fact::unknown(vec![Reason::module(
            current_module.clone(),
            "relative import does not resolve to a module path",
        )]);
    }

    let target = parts.join(".");
    match PyModuleName::parse(&target) {
        Ok(target) => Fact::known(target),
        Err(error) => Fact::unknown(vec![Reason::module(
            current_module.clone(),
            format!("relative import resolves to an invalid module path: {error}"),
        )]),
    }
}

fn push_module_search_path(
    search_paths: &mut Vec<ModuleSearchPathEntry>,
    kind: ModuleSearchPathKind,
    path: Utf8PathBuf,
) {
    if search_paths
        .iter()
        .any(|search_path| search_path.path == path)
    {
        return;
    }

    search_paths.push(ModuleSearchPathEntry { kind, path });
}

fn module_names_for_file(
    file: &Utf8Path,
    search_paths: &[ModuleSearchPathEntry],
) -> Vec<(PyModuleName, Utf8PathBuf)> {
    let Some(longest_path_len) = search_paths
        .iter()
        .filter(|search_path| file.starts_with(&search_path.path))
        .map(|search_path| search_path.path.as_str().len())
        .max()
    else {
        return Vec::new();
    };

    search_paths
        .iter()
        .filter(|search_path| {
            file.starts_with(&search_path.path)
                && search_path.path.as_str().len() == longest_path_len
        })
        .filter_map(|search_path| {
            let relative = file.strip_prefix(&search_path.path).ok()?;
            module_name_from_relative_file(relative)
                .map(|module| (module, search_path.path.clone()))
        })
        .fold(Vec::new(), |mut candidates, candidate| {
            if !candidates.iter().any(|(module, _)| module == &candidate.0) {
                candidates.push(candidate);
            }
            candidates
        })
}

fn module_name_from_relative_file(relative: &Utf8Path) -> Option<PyModuleName> {
    if relative.file_name() == Some("__init__.py") {
        return relative
            .parent()
            .filter(|parent| !parent.as_str().is_empty())
            .and_then(|parent| PyModuleName::from_relative_package(parent).ok());
    }

    PyModuleName::from_relative_python_module(relative).ok()
}

fn collect_pth_module_search_paths(
    site_packages_root: &Utf8Path,
    search_paths: &mut Vec<ModuleSearchPathEntry>,
    reasons: &mut Vec<Reason>,
) {
    let Ok(entries) = std::fs::read_dir(site_packages_root.as_std_path()) else {
        reasons.push(Reason::path(
            site_packages_root,
            "could not read site-packages directory for .pth files",
        ));
        return;
    };

    let mut pth_files = entries
        .filter_map(Result::ok)
        .filter_map(|entry| Utf8PathBuf::try_from(entry.path()).ok())
        .filter(|path| path.extension() == Some("pth") && path.is_file())
        .collect::<Vec<_>>();
    pth_files.sort();

    for pth_file in pth_files {
        let Ok(contents) = std::fs::read_to_string(pth_file.as_std_path()) else {
            reasons.push(Reason::file(
                pth_file,
                "could not read .pth module search paths",
            ));
            continue;
        };

        for line in contents.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') || line.starts_with("import ") {
                continue;
            }

            let raw_path = Utf8Path::new(line);
            let path = if raw_path.is_absolute() {
                raw_path.to_path_buf()
            } else {
                pth_file
                    .parent()
                    .unwrap_or(site_packages_root)
                    .join(raw_path)
            };

            if path.is_dir() {
                push_module_search_path(search_paths, ModuleSearchPathKind::PthFile, path);
            } else {
                reasons.push(Reason::path(
                    path,
                    ".pth entry does not exist or is not a directory",
                ));
            }
        }
    }
}

#[must_use]
fn module_candidate(
    requested: &PyModuleName,
    search_path: &ModuleSearchPathEntry,
    project_root: &Utf8Path,
) -> Option<ModuleCandidate> {
    let parts = requested.as_str().split('.').collect::<Vec<_>>();
    let module_path = parts
        .iter()
        .fold(search_path.path.clone(), |mut path, part| {
            path.push(part);
            path
        });

    let package_file = module_path.join("__init__.py");
    let module_file = module_path.with_extension("py");
    let file = if package_file.is_file() {
        package_file
    } else if module_file.is_file() {
        module_file
    } else {
        return None;
    };

    Some(ModuleCandidate {
        module: ResolvedModule {
            module: requested.clone(),
            file: file.clone(),
            search_path: search_path.path.clone(),
            location: if file.starts_with(project_root) {
                ModuleLocation::Workspace
            } else {
                ModuleLocation::External
            },
        },
    })
}

#[must_use]
fn namespace_package_reasons(parts: &[&str], search_path: &Utf8Path) -> Vec<Reason> {
    let package_segment_count = parts.len().saturating_sub(1);
    let mut dir = search_path.to_path_buf();
    let mut reasons = Vec::new();

    for part in parts.iter().take(package_segment_count) {
        dir.push(part);
        if dir.is_dir() && !dir.join("__init__.py").is_file() {
            reasons.push(Reason::path(
                dir.clone(),
                "module resolves through a namespace package segment without __init__.py",
            ));
        }
    }

    reasons
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::facts::ModuleLocation;
    use crate::project::facts::ModuleSearchPathKind;
    use crate::project::facts::ReasonSource;
    use crate::project::facts::ResolvedModule;

    fn module(name: &str) -> PyModuleName {
        PyModuleName::parse(name).unwrap()
    }

    fn write_file(path: &Utf8Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    fn search_path_entries(fact: &Fact<Vec<ModuleSearchPathEntry>>) -> &[ModuleSearchPathEntry] {
        match fact {
            Fact::Known { value } | Fact::Partial { value, .. } => value,
            Fact::Unknown { reasons } => {
                panic!("expected module search paths, got unknown: {reasons:?}")
            }
            Fact::Ambiguous {
                candidates,
                reasons,
            } => {
                panic!("expected module search paths, got ambiguous: {candidates:?} {reasons:?}")
            }
        }
    }

    fn assert_search_path(
        search_paths: &[ModuleSearchPathEntry],
        kind: ModuleSearchPathKind,
        path: &Utf8Path,
    ) {
        assert!(
            search_paths
                .iter()
                .any(|search_path| search_path.kind == kind && search_path.path == path),
            "expected {kind:?} module search path at {path}, got {search_paths:?}"
        );
    }

    fn known_module(resolution: ModuleResolution) -> ResolvedModule {
        match resolution.resolved {
            Fact::Known { value } => value,
            other => panic!("expected known module, got {other:?}"),
        }
    }

    #[test]
    fn resolves_root_layout_module() {
        let tmp = tempfile::TempDir::new().unwrap();
        let project_root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&project_root.join("blog/__init__.py"), "");
        write_file(&project_root.join("blog/models.py"), "");

        let search_paths = discover_module_search_paths(&project_root, &[], &[]);
        let roots = search_path_entries(&search_paths);
        assert_search_path(roots, ModuleSearchPathKind::Workspace, &project_root);

        let resolved = known_module(resolve_module(module("blog.models"), roots, &project_root));

        assert_eq!(resolved.module, module("blog.models"));
        assert_eq!(resolved.file, project_root.join("blog/models.py"));
        assert_eq!(resolved.search_path, project_root);
        assert_eq!(resolved.location, ModuleLocation::Workspace);
    }

    #[test]
    fn discovers_top_level_src_search_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let project_root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&project_root.join("src/config/__init__.py"), "");
        write_file(&project_root.join("src/config/settings.py"), "");

        let search_paths = discover_module_search_paths(&project_root, &[], &[]);
        let roots = search_path_entries(&search_paths);
        let src_root = project_root.join("src");
        assert_search_path(roots, ModuleSearchPathKind::AutoSrc, &src_root);

        let resolved = known_module(resolve_module(
            module("config.settings"),
            roots,
            &project_root,
        ));

        assert_eq!(resolved.file, src_root.join("config/settings.py"));
        assert_eq!(resolved.search_path, src_root);
    }

    #[test]
    fn resolves_module_name_for_file_from_longest_search_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let project_root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&project_root.join("src/config/__init__.py"), "");
        write_file(&project_root.join("src/config/settings.py"), "");

        let search_paths = discover_module_search_paths(&project_root, &[], &[]);
        let fact = module_name_for_file(
            &project_root.join("src/config/settings.py"),
            search_path_entries(&search_paths),
        );

        let Fact::Known { value } = fact else {
            panic!("expected known module for settings file, got {fact:?}");
        };
        assert_eq!(value, module("config.settings"));
    }

    #[test]
    fn resolves_namespace_package_module_name_for_file_as_known() {
        let tmp = tempfile::TempDir::new().unwrap();
        let project_root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&project_root.join("src/acme/plugins/blog/apps.py"), "");

        let search_paths = discover_module_search_paths(&project_root, &[], &[]);
        let fact = module_name_for_file(
            &project_root.join("src/acme/plugins/blog/apps.py"),
            search_path_entries(&search_paths),
        );

        let Fact::Known { value } = fact else {
            panic!("expected known module for namespace package file, got {fact:?}");
        };
        assert_eq!(value, module("acme.plugins.blog.apps"));
    }

    #[test]
    fn uses_explicit_nested_source_roots() {
        let tmp = tempfile::TempDir::new().unwrap();
        let project_root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let source_root = project_root.join("backend/python/apps");
        write_file(&source_root.join("catalog/__init__.py"), "");
        write_file(&source_root.join("catalog/apps.py"), "");

        let search_paths = discover_module_search_paths(
            &project_root,
            &[Utf8PathBuf::from("backend/python/apps")],
            &[],
        );
        let roots = search_path_entries(&search_paths);
        assert_search_path(
            roots,
            ModuleSearchPathKind::ExplicitPythonPath,
            &source_root,
        );

        let resolved = known_module(resolve_module(module("catalog.apps"), roots, &project_root));

        assert_eq!(resolved.file, source_root.join("catalog/apps.py"));
        assert_eq!(resolved.search_path, source_root);
    }

    #[test]
    fn duplicate_modules_are_ambiguous() {
        let tmp = tempfile::TempDir::new().unwrap();
        let project_root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&project_root.join("shop/__init__.py"), "");
        write_file(&project_root.join("shop/apps.py"), "");
        write_file(&project_root.join("src/shop/__init__.py"), "");
        write_file(&project_root.join("src/shop/apps.py"), "");

        let search_paths = discover_module_search_paths(&project_root, &[], &[]);
        let resolution = resolve_module(
            module("shop.apps"),
            search_path_entries(&search_paths),
            &project_root,
        );

        match resolution.resolved {
            Fact::Ambiguous {
                candidates,
                reasons,
            } => {
                assert_eq!(candidates.len(), 2);
                assert!(candidates
                    .iter()
                    .any(|candidate| candidate.file == project_root.join("shop/apps.py")));
                assert!(candidates
                    .iter()
                    .any(|candidate| candidate.file == project_root.join("src/shop/apps.py")));
                assert!(reasons
                    .iter()
                    .any(|reason| matches!(&reason.source, ReasonSource::Module(_))));
            }
            other => panic!("expected ambiguous duplicate module, got {other:?}"),
        }
    }

    #[test]
    fn namespace_package_resolution_is_known_when_unique() {
        let tmp = tempfile::TempDir::new().unwrap();
        let project_root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&project_root.join("src/acme/plugins/blog/apps.py"), "");

        let search_paths = discover_module_search_paths(&project_root, &[], &[]);
        let resolution = resolve_module(
            module("acme.plugins.blog.apps"),
            search_path_entries(&search_paths),
            &project_root,
        );

        match resolution.resolved {
            Fact::Known { value } => {
                assert_eq!(
                    value.file,
                    project_root.join("src/acme/plugins/blog/apps.py")
                );
            }
            other => panic!("expected known namespace package module, got {other:?}"),
        }
    }

    #[test]
    fn discovers_pth_search_paths() {
        let project_tmp = tempfile::TempDir::new().unwrap();
        let project_root = Utf8PathBuf::try_from(project_tmp.path().to_path_buf()).unwrap();
        let site_tmp = tempfile::TempDir::new().unwrap();
        let site_root = Utf8PathBuf::try_from(site_tmp.path().to_path_buf()).unwrap();
        let site_packages = site_root.join("lib/python3.12/site-packages");
        let pth_root = project_root.join("vendor_src");
        write_file(&pth_root.join("vendored_app/__init__.py"), "");
        write_file(&pth_root.join("vendored_app/apps.py"), "");
        write_file(
            &site_packages.join("workspace.pth"),
            "# generated by editable install\n../../../ignored\nimport site\n",
        );
        write_file(
            &site_packages.join("editable.pth"),
            &format!("{pth_root}\n"),
        );

        let search_paths =
            discover_module_search_paths(&project_root, &[], std::slice::from_ref(&site_packages));
        let roots = search_path_entries(&search_paths);
        assert_search_path(roots, ModuleSearchPathKind::SitePackages, &site_packages);
        assert_search_path(roots, ModuleSearchPathKind::PthFile, &pth_root);

        let resolved = known_module(resolve_module(
            module("vendored_app.apps"),
            roots,
            &project_root,
        ));

        assert_eq!(resolved.file, pth_root.join("vendored_app/apps.py"));
        assert_eq!(resolved.search_path, pth_root);
    }

    #[test]
    fn resolves_relative_import_from_split_settings() {
        let tmp = tempfile::TempDir::new().unwrap();
        let project_root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&project_root.join("projects/__init__.py"), "");
        write_file(&project_root.join("projects/site1/__init__.py"), "");
        write_file(
            &project_root.join("projects/site1/settings/__init__.py"),
            "",
        );
        write_file(&project_root.join("projects/site1/settings/base.py"), "");
        write_file(
            &project_root.join("projects/site1/settings/dev.py"),
            "from .base import *\n",
        );

        let target =
            resolve_relative_import_module(&module("projects.site1.settings.dev"), 1, Some("base"));
        let Fact::Known { value: target } = target else {
            panic!("expected known relative import target, got {target:?}");
        };
        assert_eq!(target, module("projects.site1.settings.base"));

        let search_paths = discover_module_search_paths(&project_root, &[], &[]);
        let resolved = known_module(resolve_module(
            target,
            search_path_entries(&search_paths),
            &project_root,
        ));

        assert_eq!(
            resolved.file,
            project_root.join("projects/site1/settings/base.py")
        );
    }

    #[test]
    fn rejects_invalid_relative_import_levels() {
        let zero = resolve_relative_import_module(&module("project.settings.dev"), 0, Some("base"));
        assert!(matches!(zero, Fact::Unknown { .. }));

        let overflow =
            resolve_relative_import_module(&module("project.settings.dev"), 4, Some("base"));
        assert!(matches!(overflow, Fact::Unknown { .. }));
    }
}

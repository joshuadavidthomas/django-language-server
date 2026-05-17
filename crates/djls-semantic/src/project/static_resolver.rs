//! Static Python module resolver for the Django project model.
//!
//! This module is intentionally not wired into validation yet. It provides the
//! first native resolver slice behind confidence-aware facts so later static
//! model milestones can compose settings, apps, and template discovery without
//! depending on the runtime inspector.

#![allow(
    dead_code,
    reason = "Milestone A2 adds the native resolver before wiring it into project assembly."
)]

use camino::Utf8Path;
use camino::Utf8PathBuf;

use crate::project::names::PyModuleName;
use crate::project::static_model::Fact;
use crate::project::static_model::Field;
use crate::project::static_model::ImportRoot;
use crate::project::static_model::ImportRootKind;
use crate::project::static_model::ModuleLocation;
use crate::project::static_model::ModuleResolution;
use crate::project::static_model::Reason;
use crate::project::static_model::ResolvedModule;

struct ModuleCandidate {
    module: ResolvedModule,
    reasons: Vec<Reason>,
}

#[must_use]
pub(crate) fn discover_import_roots(
    project_root: &Utf8Path,
    explicit_roots: &[Utf8PathBuf],
    site_packages_roots: &[Utf8PathBuf],
) -> Fact<Vec<ImportRoot>> {
    let mut roots = Vec::new();
    let mut reasons = Vec::new();

    if project_root.is_dir() {
        push_import_root(
            &mut roots,
            ImportRootKind::Workspace,
            project_root.to_path_buf(),
        );
    } else {
        reasons.push(Reason::path(
            Field::ResolverImportRoots,
            project_root,
            "project root does not exist or is not a directory",
        ));
    }

    let src_root = project_root.join("src");
    if src_root.is_dir() {
        push_import_root(&mut roots, ImportRootKind::AutoSrc, src_root);
    }

    for explicit_root in explicit_roots {
        let root = if explicit_root.is_absolute() {
            explicit_root.clone()
        } else {
            project_root.join(explicit_root)
        };
        if root.is_dir() {
            push_import_root(&mut roots, ImportRootKind::ExplicitPythonPath, root);
        } else {
            reasons.push(Reason::path(
                Field::ResolverImportRoots,
                root,
                "explicit Python import root does not exist or is not a directory",
            ));
        }
    }

    for site_packages_root in site_packages_roots {
        if site_packages_root.is_dir() {
            push_import_root(
                &mut roots,
                ImportRootKind::SitePackages,
                site_packages_root.clone(),
            );
            collect_pth_import_roots(site_packages_root, &mut roots, &mut reasons);
        } else {
            reasons.push(Reason::path(
                Field::ResolverImportRoots,
                site_packages_root,
                "site-packages import root does not exist or is not a directory",
            ));
        }
    }

    if roots.is_empty() {
        Fact::unknown(reasons)
    } else if reasons.is_empty() {
        Fact::known(roots)
    } else {
        Fact::partial(roots, reasons)
    }
}

#[must_use]
pub(crate) fn resolve_module(
    requested: PyModuleName,
    import_roots: &[ImportRoot],
    project_root: &Utf8Path,
) -> ModuleResolution {
    let mut candidates = import_roots
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
            Field::ResolverModule,
            requested.clone(),
            "module was not found in import roots",
        )]),
        1 => {
            let candidate = candidates.pop().unwrap();
            if candidate.reasons.is_empty() {
                Fact::known(candidate.module)
            } else {
                Fact::partial(candidate.module, candidate.reasons)
            }
        }
        _ => Fact::ambiguous(
            candidates
                .into_iter()
                .map(|candidate| candidate.module)
                .collect(),
            vec![Reason::module(
                Field::ResolverModule,
                requested.clone(),
                "module resolves to more than one import root",
            )],
        ),
    };

    ModuleResolution {
        requested,
        resolved,
    }
}

#[must_use]
pub(crate) fn resolve_file_module(
    file: &Utf8Path,
    import_roots: &[ImportRoot],
) -> Fact<PyModuleName> {
    if !file.is_file() {
        return Fact::unknown(vec![Reason::file(
            Field::ResolverModule,
            file,
            "module file does not exist or is not a file",
        )]);
    }

    if file.extension() != Some("py") {
        return Fact::unknown(vec![Reason::file(
            Field::ResolverModule,
            file,
            "module file is not a Python file",
        )]);
    }

    let candidates = module_names_for_file(file, import_roots);
    match candidates.len() {
        0 => Fact::unknown(vec![Reason::file(
            Field::ResolverModule,
            file,
            "module file is outside configured import roots",
        )]),
        1 => {
            let (module, import_root) = candidates.into_iter().next().unwrap();
            let parts = module.as_str().split('.').collect::<Vec<_>>();
            let reasons = namespace_package_reasons(&parts, &import_root);
            if reasons.is_empty() {
                Fact::known(module)
            } else {
                Fact::partial(module, reasons)
            }
        }
        _ => Fact::ambiguous(
            candidates.into_iter().map(|(module, _)| module).collect(),
            vec![Reason::file(
                Field::ResolverModule,
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
            Field::ResolverRelativeImport,
            current_module.clone(),
            "relative import level must be greater than zero",
        )]);
    }

    let mut parts = current_module.as_str().split('.').collect::<Vec<_>>();
    parts.pop();

    if level > parts.len() + 1 {
        return Fact::unknown(vec![Reason::module(
            Field::ResolverRelativeImport,
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
            Field::ResolverRelativeImport,
            current_module.clone(),
            "relative import does not resolve to a module path",
        )]);
    }

    let target = parts.join(".");
    match PyModuleName::parse(&target) {
        Ok(target) => Fact::known(target),
        Err(error) => Fact::unknown(vec![Reason::module(
            Field::ResolverRelativeImport,
            current_module.clone(),
            format!("relative import resolves to an invalid module path: {error}"),
        )]),
    }
}

fn push_import_root(roots: &mut Vec<ImportRoot>, kind: ImportRootKind, path: Utf8PathBuf) {
    if roots.iter().any(|root| root.path == path) {
        return;
    }

    roots.push(ImportRoot { kind, path });
}

fn module_names_for_file(
    file: &Utf8Path,
    import_roots: &[ImportRoot],
) -> Vec<(PyModuleName, Utf8PathBuf)> {
    let Some(longest_root_len) = import_roots
        .iter()
        .filter(|root| file.starts_with(&root.path))
        .map(|root| root.path.as_str().len())
        .max()
    else {
        return Vec::new();
    };

    import_roots
        .iter()
        .filter(|root| file.starts_with(&root.path) && root.path.as_str().len() == longest_root_len)
        .filter_map(|root| {
            let relative = file.strip_prefix(&root.path).ok()?;
            module_name_from_relative_file(relative).map(|module| (module, root.path.clone()))
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

fn collect_pth_import_roots(
    site_packages_root: &Utf8Path,
    roots: &mut Vec<ImportRoot>,
    reasons: &mut Vec<Reason>,
) {
    let Ok(entries) = std::fs::read_dir(site_packages_root.as_std_path()) else {
        reasons.push(Reason::path(
            Field::ResolverImportRoots,
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
                Field::ResolverImportRoots,
                pth_file,
                "could not read .pth import roots",
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
                push_import_root(roots, ImportRootKind::PthFile, path);
            } else {
                reasons.push(Reason::path(
                    Field::ResolverImportRoots,
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
    import_root: &ImportRoot,
    project_root: &Utf8Path,
) -> Option<ModuleCandidate> {
    let parts = requested.as_str().split('.').collect::<Vec<_>>();
    let module_path = parts
        .iter()
        .fold(import_root.path.clone(), |mut path, part| {
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

    let reasons = namespace_package_reasons(&parts, &import_root.path);
    Some(ModuleCandidate {
        module: ResolvedModule {
            module: requested.clone(),
            file: file.clone(),
            import_root: import_root.path.clone(),
            location: if file.starts_with(project_root) {
                ModuleLocation::Workspace
            } else {
                ModuleLocation::External
            },
        },
        reasons,
    })
}

#[must_use]
fn namespace_package_reasons(parts: &[&str], import_root: &Utf8Path) -> Vec<Reason> {
    let package_segment_count = parts.len().saturating_sub(1);
    let mut dir = import_root.to_path_buf();
    let mut reasons = Vec::new();

    for part in parts.iter().take(package_segment_count) {
        dir.push(part);
        if dir.is_dir() && !dir.join("__init__.py").is_file() {
            reasons.push(Reason::path(
                Field::ResolverModule,
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
    use crate::project::static_model::Field;
    use crate::project::static_model::ImportRootKind;
    use crate::project::static_model::ModuleLocation;
    use crate::project::static_model::ResolvedModule;

    fn module(name: &str) -> PyModuleName {
        PyModuleName::parse(name).unwrap()
    }

    fn write_file(path: &Utf8Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    fn roots_value(fact: &Fact<Vec<ImportRoot>>) -> &[ImportRoot] {
        match fact {
            Fact::Known { value } | Fact::Partial { value, .. } => value,
            Fact::Unknown { reasons } => panic!("expected roots, got unknown: {reasons:?}"),
            Fact::Ambiguous {
                candidates,
                reasons,
            } => {
                panic!("expected roots, got ambiguous: {candidates:?} {reasons:?}")
            }
        }
    }

    fn assert_root(roots: &[ImportRoot], kind: ImportRootKind, path: &Utf8Path) {
        assert!(
            roots
                .iter()
                .any(|root| root.kind == kind && root.path == path),
            "expected {kind:?} root at {path}, got {roots:?}"
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

        let import_roots = discover_import_roots(&project_root, &[], &[]);
        let roots = roots_value(&import_roots);
        assert_root(roots, ImportRootKind::Workspace, &project_root);

        let resolved = known_module(resolve_module(module("blog.models"), roots, &project_root));

        assert_eq!(resolved.module, module("blog.models"));
        assert_eq!(resolved.file, project_root.join("blog/models.py"));
        assert_eq!(resolved.import_root, project_root);
        assert_eq!(resolved.location, ModuleLocation::Workspace);
    }

    #[test]
    fn discovers_top_level_src_import_root() {
        let tmp = tempfile::TempDir::new().unwrap();
        let project_root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&project_root.join("src/config/__init__.py"), "");
        write_file(&project_root.join("src/config/settings.py"), "");

        let import_roots = discover_import_roots(&project_root, &[], &[]);
        let roots = roots_value(&import_roots);
        let src_root = project_root.join("src");
        assert_root(roots, ImportRootKind::AutoSrc, &src_root);

        let resolved = known_module(resolve_module(
            module("config.settings"),
            roots,
            &project_root,
        ));

        assert_eq!(resolved.file, src_root.join("config/settings.py"));
        assert_eq!(resolved.import_root, src_root);
    }

    #[test]
    fn resolves_file_module_from_longest_import_root() {
        let tmp = tempfile::TempDir::new().unwrap();
        let project_root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&project_root.join("src/config/__init__.py"), "");
        write_file(&project_root.join("src/config/settings.py"), "");

        let import_roots = discover_import_roots(&project_root, &[], &[]);
        let fact = resolve_file_module(
            &project_root.join("src/config/settings.py"),
            roots_value(&import_roots),
        );

        let Fact::Known { value } = fact else {
            panic!("expected known module for settings file, got {fact:?}");
        };
        assert_eq!(value, module("config.settings"));
    }

    #[test]
    fn uses_explicit_nested_source_roots() {
        let tmp = tempfile::TempDir::new().unwrap();
        let project_root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        let source_root = project_root.join("backend/python/apps");
        write_file(&source_root.join("catalog/__init__.py"), "");
        write_file(&source_root.join("catalog/apps.py"), "");

        let import_roots = discover_import_roots(
            &project_root,
            &[Utf8PathBuf::from("backend/python/apps")],
            &[],
        );
        let roots = roots_value(&import_roots);
        assert_root(roots, ImportRootKind::ExplicitPythonPath, &source_root);

        let resolved = known_module(resolve_module(module("catalog.apps"), roots, &project_root));

        assert_eq!(resolved.file, source_root.join("catalog/apps.py"));
        assert_eq!(resolved.import_root, source_root);
    }

    #[test]
    fn duplicate_modules_are_ambiguous() {
        let tmp = tempfile::TempDir::new().unwrap();
        let project_root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&project_root.join("shop/__init__.py"), "");
        write_file(&project_root.join("shop/apps.py"), "");
        write_file(&project_root.join("src/shop/__init__.py"), "");
        write_file(&project_root.join("src/shop/apps.py"), "");

        let import_roots = discover_import_roots(&project_root, &[], &[]);
        let resolution = resolve_module(
            module("shop.apps"),
            roots_value(&import_roots),
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
                    .any(|reason| reason.field == Field::ResolverModule));
            }
            other => panic!("expected ambiguous duplicate module, got {other:?}"),
        }
    }

    #[test]
    fn namespace_package_resolution_is_partial() {
        let tmp = tempfile::TempDir::new().unwrap();
        let project_root = Utf8PathBuf::try_from(tmp.path().to_path_buf()).unwrap();
        write_file(&project_root.join("src/acme/plugins/blog/apps.py"), "");

        let import_roots = discover_import_roots(&project_root, &[], &[]);
        let resolution = resolve_module(
            module("acme.plugins.blog.apps"),
            roots_value(&import_roots),
            &project_root,
        );

        match resolution.resolved {
            Fact::Partial { value, reasons } => {
                assert_eq!(
                    value.file,
                    project_root.join("src/acme/plugins/blog/apps.py")
                );
                assert!(reasons
                    .iter()
                    .any(|reason| reason.field == Field::ResolverModule));
            }
            other => panic!("expected partial namespace package module, got {other:?}"),
        }
    }

    #[test]
    fn discovers_pth_import_roots() {
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

        let import_roots =
            discover_import_roots(&project_root, &[], std::slice::from_ref(&site_packages));
        let roots = roots_value(&import_roots);
        assert_root(roots, ImportRootKind::SitePackages, &site_packages);
        assert_root(roots, ImportRootKind::PthFile, &pth_root);

        let resolved = known_module(resolve_module(
            module("vendored_app.apps"),
            roots,
            &project_root,
        ));

        assert_eq!(resolved.file, pth_root.join("vendored_app/apps.py"));
        assert_eq!(resolved.import_root, pth_root);
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

        let import_roots = discover_import_roots(&project_root, &[], &[]);
        let resolved = known_module(resolve_module(
            target,
            roots_value(&import_roots),
            &project_root,
        ));

        assert_eq!(
            resolved.file,
            project_root.join("projects/site1/settings/base.py")
        );
    }
}

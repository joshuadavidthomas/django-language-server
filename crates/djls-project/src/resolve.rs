//! Module name → file path resolution using Python search paths.

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::File;
use djls_source::FileRootKind;
use djls_source::FileSystem;
use djls_source::WalkEntryKind;
use djls_source::WalkOptions;
use rustc_hash::FxHashMap;

use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonModule;
use crate::python::PythonModuleName;

#[salsa::tracked(returns(ref))]
pub fn model_modules(db: &dyn ProjectDb, project: Project) -> Vec<PythonModule> {
    let search_paths = project.search_paths(db);
    let mut modules_by_path: Vec<(usize, PythonModule)> = Vec::new();
    let mut path_indexes: FxHashMap<Utf8PathBuf, usize> = FxHashMap::default();

    for search_path in search_paths.iter() {
        if let Some(root) = db.files().root(db, search_path.path()) {
            let _ = root.revision(db);
        } else {
            tracing::warn!(
                "Search path has no registered source root: {}",
                search_path.path()
            );
        }

        let excluded_paths: Vec<_> = if search_path.is_first_party() {
            search_paths
                .iter()
                .filter(|other| {
                    !other.is_first_party() && other.path().starts_with(search_path.path())
                })
                .map(|other| other.path().to_path_buf())
                .collect()
        } else {
            Vec::new()
        };

        let search_path_len = search_path.path().as_str().len();
        for (module_name, file_path) in discover_model_files_in_root(
            db.file_system(),
            search_path.path(),
            search_path.root_kind(),
            &excluded_paths,
        ) {
            let module = PythonModule::new(
                module_name,
                file_path.clone(),
                db.get_or_create_file(&file_path),
            );
            if let Some(index) = path_indexes.get(&file_path).copied() {
                let (existing_search_path_len, existing) = &mut modules_by_path[index];
                if search_path_len > *existing_search_path_len {
                    *existing_search_path_len = search_path_len;
                    *existing = module;
                }
            } else {
                path_indexes.insert(file_path, modules_by_path.len());
                modules_by_path.push((search_path_len, module));
            }
        }
    }

    modules_by_path
        .into_iter()
        .map(|(_search_path_len, module)| module)
        .collect()
}

#[derive(Clone, Copy)]
pub(crate) struct ImportParts<'a> {
    pub level: u32,
    pub module: Option<&'a str>,
    pub importer: &'a Utf8Path,
}

pub(crate) fn python_module(
    db: &dyn ProjectDb,
    project: Project,
    module_name: PythonModuleName,
) -> Option<PythonModule> {
    project.touch_search_path_roots(db);

    for search_path in project.search_paths(db).iter() {
        let Some(path) =
            module_file_in_search_path(db.file_system(), module_name.as_str(), search_path.path())
        else {
            continue;
        };
        let file = db.get_or_create_file(&path);
        return Some(PythonModule::new(module_name, path, file));
    }

    None
}

pub(crate) fn module_file(db: &dyn ProjectDb, project: Project, module_name: &str) -> Option<File> {
    project.touch_search_path_roots(db);

    for search_path in project.search_paths(db).iter() {
        let Some(path) =
            module_file_in_search_path(db.file_system(), module_name, search_path.path())
        else {
            continue;
        };
        return Some(db.get_or_create_file(&path));
    }

    None
}

pub(crate) fn package_dir(
    db: &dyn ProjectDb,
    project: Project,
    package_module: &str,
) -> Option<Utf8PathBuf> {
    if package_module.is_empty() {
        return None;
    }

    let relative = package_module.replace('.', "/");
    for search_path in project.search_paths(db).iter() {
        let candidate = search_path.path().join(&relative);
        if db.path_is_dir(&candidate) {
            return Some(candidate);
        }
    }

    None
}

pub(crate) fn resolve_import(
    db: &dyn ProjectDb,
    project: Project,
    parts: ImportParts<'_>,
) -> Option<String> {
    if parts.level == 0 {
        return parts.module.map(str::to_string);
    }

    let root = project
        .search_paths(db)
        .iter()
        .filter(|search_path| parts.importer.starts_with(search_path.path()))
        .max_by_key(|search_path| search_path.path().as_str().len())?
        .path();
    let relative = parts.importer.strip_prefix(root).ok()?;
    if relative.extension() != Some("py") {
        return None;
    }

    let mut module_parts: Vec<String> = relative
        .parent()?
        .components()
        .map(|component| component.as_str().to_string())
        .collect();

    for _ in 1..parts.level {
        module_parts.pop()?;
    }

    if let Some(module) = parts.module {
        module_parts.extend(
            module
                .split('.')
                .filter(|part| !part.is_empty())
                .map(str::to_string),
        );
    }

    (!module_parts.is_empty()).then(|| module_parts.join("."))
}

fn module_file_in_search_path(
    fs: &dyn FileSystem,
    module_name: &str,
    search_path: &Utf8Path,
) -> Option<Utf8PathBuf> {
    let mut candidate = search_path.to_path_buf();
    for part in module_name.split('.') {
        candidate.push(part);
    }

    let py_file = candidate.with_extension("py");
    if fs.is_file(&py_file) {
        return Some(py_file);
    }

    let init_file = candidate.join("__init__.py");
    fs.is_file(&init_file).then_some(init_file)
}

/// Discover Django model source files under one Python search root.
///
/// Finds `models.py` files and `.py` files inside `models/` packages
/// (directories with `__init__.py`) without reading file contents.
fn discover_model_files_in_root(
    fs: &dyn FileSystem,
    base_dir: &Utf8Path,
    root_kind: FileRootKind,
    excluded_roots: &[Utf8PathBuf],
) -> Vec<(PythonModuleName, Utf8PathBuf)> {
    let options = match root_kind {
        FileRootKind::Project => WalkOptions::project(),
        FileRootKind::SearchPath => WalkOptions::library_search_path(),
    };

    let mut results = Vec::new();

    let entries = match fs.walk_entries(base_dir, &options) {
        Ok(entries) => entries,
        Err(err) => {
            tracing::warn!("Failed to walk Python source root {}: {}", base_dir, err);
            return results;
        }
    };

    for entry in entries {
        if entry.kind != WalkEntryKind::File {
            continue;
        }
        let path = entry.path;

        let is_django_model_source = if path.file_name() == Some("models.py") {
            true
        } else if path.extension() == Some("py") {
            let mut dir = path.parent();
            let mut in_models_package = false;
            while let Some(parent) = dir {
                if parent.file_name() == Some("models") {
                    in_models_package = fs.exists(&parent.join("__init__.py"));
                    break;
                }
                dir = parent.parent();
            }
            in_models_package
        } else {
            false
        };

        if !is_django_model_source {
            continue;
        }

        if excluded_roots
            .iter()
            .any(|excluded| path.starts_with(excluded))
        {
            continue;
        }

        let Some(rel) = path.strip_prefix(base_dir).ok() else {
            continue;
        };

        let Ok(module_name) = PythonModuleName::from_relative_source_path(rel) else {
            continue;
        };

        results.push((module_name, path));
    }

    results.sort_by(|(a, _), (b, _)| a.cmp(b));
    results
}

//! Module path → file path resolution using Python search paths.

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
use crate::python::Interpreter;
use crate::python::PythonModule;
use crate::python::PythonModulePath;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchPath {
    FirstParty(Utf8PathBuf),
    Extra(Utf8PathBuf),
    SitePackages(Utf8PathBuf),
}

impl SearchPath {
    fn first_party(path: Utf8PathBuf) -> Self {
        Self::FirstParty(path)
    }

    fn extra(path: Utf8PathBuf) -> Self {
        Self::Extra(path)
    }

    fn site_packages(path: Utf8PathBuf) -> Self {
        Self::SitePackages(path)
    }

    fn from_pythonpath(
        root: &Utf8Path,
        discovered_site_packages: Option<&Utf8Path>,
        path: Utf8PathBuf,
    ) -> Self {
        if discovered_site_packages.is_some_and(|site_packages| site_packages == path)
            || has_installed_package_component(&path)
        {
            Self::site_packages(path)
        } else if path.starts_with(root) {
            Self::first_party(path)
        } else {
            Self::extra(path)
        }
    }

    #[must_use]
    pub fn path(&self) -> &Utf8Path {
        match self {
            Self::FirstParty(path) | Self::Extra(path) | Self::SitePackages(path) => path,
        }
    }

    #[must_use]
    pub(crate) fn is_first_party(&self) -> bool {
        matches!(self, Self::FirstParty(_))
    }

    pub(crate) fn root_kind(&self) -> FileRootKind {
        match self {
            // Extra pythonpath entries are user-edited code, so they get the
            // same low-durability treatment as project files.
            Self::FirstParty(_) | Self::Extra(_) => FileRootKind::Project,
            Self::SitePackages(_) => FileRootKind::SearchPath,
        }
    }
}

fn has_installed_package_component(path: &Utf8Path) -> bool {
    path.components()
        .any(|component| matches!(component.as_str(), "site-packages" | "dist-packages"))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchPaths {
    paths: Vec<SearchPath>,
}

impl SearchPaths {
    #[must_use]
    pub(crate) fn root_only(root: &Utf8Path) -> Self {
        let mut search_paths = Self::default();
        search_paths.push(SearchPath::first_party(root.to_path_buf()));
        search_paths
    }

    #[must_use]
    pub fn from_project_settings(
        fs: &dyn FileSystem,
        root: &Utf8Path,
        interpreter: &Interpreter,
        pythonpath: &[String],
    ) -> Self {
        let mut search_paths = Self::default();
        search_paths.push(SearchPath::first_party(root.to_path_buf()));
        let discovered_site_packages = interpreter.site_packages_path(fs, root);

        for path in pythonpath {
            let path = Utf8PathBuf::from(path);
            if !fs.is_dir(&path) || search_paths.contains_path(&path) {
                continue;
            }

            search_paths.push(SearchPath::from_pythonpath(
                root,
                discovered_site_packages.as_deref(),
                path,
            ));
        }

        if let Some(site_packages) = discovered_site_packages
            && !search_paths.contains_path(&site_packages)
        {
            search_paths.push(SearchPath::site_packages(site_packages));
        }

        search_paths
    }

    pub fn register_roots(&self, db: &dyn ProjectDb) {
        let mut roots = Vec::new();
        for search_path in self.iter() {
            if search_path.is_first_party()
                && roots.iter().any(|(path, kind)| {
                    *kind == FileRootKind::Project && search_path.path().starts_with(path)
                })
            {
                continue;
            }

            roots.push((search_path.path().to_path_buf(), search_path.root_kind()));
        }

        db.files().replace_roots(db, roots);
    }

    pub fn iter(&self) -> impl Iterator<Item = &SearchPath> {
        self.paths.iter()
    }

    fn push(&mut self, search_path: SearchPath) {
        self.paths.push(search_path);
    }

    fn contains_path(&self, path: &Utf8Path) -> bool {
        self.iter().any(|search_path| search_path.path() == path)
    }
}

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
        for (module_path, file_path) in discover_model_files_in_root(
            db.file_system(),
            search_path.path(),
            search_path.root_kind(),
            &excluded_paths,
        ) {
            let module = PythonModule::new(
                module_path,
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
    module_path: PythonModulePath,
) -> Option<PythonModule> {
    project.touch_search_path_roots(db);

    for search_path in project.search_paths(db).iter() {
        let Some(path) =
            module_file_in_search_path(db.file_system(), module_path.as_str(), search_path.path())
        else {
            continue;
        };
        let file = db.get_or_create_file(&path);
        return Some(PythonModule::new(module_path, path, file));
    }

    None
}

pub(crate) fn module_file(db: &dyn ProjectDb, project: Project, module_path: &str) -> Option<File> {
    project.touch_search_path_roots(db);

    for search_path in project.search_paths(db).iter() {
        let Some(path) =
            module_file_in_search_path(db.file_system(), module_path, search_path.path())
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
    module_path: &str,
    search_path: &Utf8Path,
) -> Option<Utf8PathBuf> {
    let mut candidate = search_path.to_path_buf();
    for part in module_path.split('.') {
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
) -> Vec<(PythonModulePath, Utf8PathBuf)> {
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

        let Ok(module_path) = PythonModulePath::from_relative_python_module(rel) else {
            continue;
        };

        results.push((module_path, path));
    }

    results.sort_by(|(a, _), (b, _)| a.cmp(b));
    results
}

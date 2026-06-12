//! Module path → file path resolution using Python search paths.

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::FileRootKind;
use djls_source::FileSystem;
use djls_source::WalkEntryKind;
use djls_source::WalkOptions;
use rustc_hash::FxHashMap;

use crate::db::Db as ProjectDb;
use crate::names::ModulePath;
use crate::project::Project;
use crate::project::PythonModule;
use crate::python::Interpreter;

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
        for (module_path, file_path) in discover_model_files_excluding(
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

pub(crate) fn module_file_in_search_path(
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

#[salsa::tracked(returns(ref))]
pub fn templatetag_modules(db: &dyn ProjectDb, project: Project) -> Vec<PythonModule> {
    let search_paths: Vec<_> = project.search_paths(db).iter().collect();
    for search_path in &search_paths {
        if let Some(root) = db.files().root(db, search_path.path()) {
            let _ = root.revision(db);
        } else {
            tracing::warn!(
                "Search path has no registered source root: {}",
                search_path.path()
            );
        }
    }

    let mut modules = Vec::new();

    for (module_index, module_path) in crate::settings::template_libraries(db, project)
        .registration_modules()
        .into_iter()
        .enumerate()
    {
        for search_path in &search_paths {
            let Some(file_path) = module_file_in_search_path(
                db.file_system(),
                module_path.as_str(),
                search_path.path(),
            ) else {
                continue;
            };

            modules.push((
                module_index,
                PythonModule::new(
                    ModulePath::new(module_path.as_str().to_string()),
                    file_path.clone(),
                    db.get_or_create_file(&file_path),
                ),
            ));
            break;
        }
    }

    modules.sort_by(|(left_index, left), (right_index, right)| {
        left_index
            .cmp(right_index)
            .then_with(|| left.module_path().cmp(right.module_path()))
            .then_with(|| left.path().cmp(right.path()))
    });

    modules.into_iter().map(|(_index, module)| module).collect()
}

/// Discover Django model source files and return their resolved module paths.
///
/// Finds `models.py` files and `.py` files inside `models/` packages
/// (directories with `__init__.py`) without reading file contents.
#[must_use]
pub fn discover_model_files(
    fs: &dyn FileSystem,
    base_dir: &Utf8Path,
    root_kind: FileRootKind,
) -> Vec<(ModulePath, Utf8PathBuf)> {
    discover_model_files_excluding(fs, base_dir, root_kind, &[])
}

fn discover_model_files_excluding(
    fs: &dyn FileSystem,
    base_dir: &Utf8Path,
    root_kind: FileRootKind,
    excluded_roots: &[Utf8PathBuf],
) -> Vec<(ModulePath, Utf8PathBuf)> {
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

        results.push((ModulePath::from_relative_path(rel), path));
    }

    results.sort_by(|(a, _), (b, _)| a.cmp(b));
    results
}

use std::fmt;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::File;
use thiserror::Error;

use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::InvalidModuleName;
use crate::python::PythonModuleName;
use crate::python::search_paths::SearchPath;

#[derive(Clone, PartialEq, Eq)]
pub struct PythonModule {
    name: PythonModuleName,
    path: Utf8PathBuf,
    file: File,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PythonPackage {
    name: PythonModuleName,
    dir: Utf8PathBuf,
    init_file: Option<File>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolutionDetail {
    pub selected_root: Option<SearchPath>,
    pub candidates: Vec<CandidateLocation>,
    pub unresolved_reason: Option<UnresolvedReason>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidateLocation {
    pub root: SearchPath,
    pub path: Utf8PathBuf,
    pub kind: CandidateKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateKind {
    RegularPackage,
    FileModule,
    NamespacePortion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnresolvedReason {
    NotFound,
    NamespaceOnly,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PackageDirs {
    pub dirs: Vec<Utf8PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PythonModuleCandidate {
    RegularPackage {
        root: SearchPath,
        dir: Utf8PathBuf,
        init_file: Utf8PathBuf,
    },
    FileModule {
        root: SearchPath,
        path: Utf8PathBuf,
    },
    NamespacePortion {
        root: SearchPath,
        dir: Utf8PathBuf,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PythonImport<'a> {
    pub(crate) level: u32,
    pub(crate) module: Option<&'a str>,
    pub(crate) importer: &'a Utf8Path,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub(crate) enum PythonImportError {
    #[error(transparent)]
    InvalidModuleName(#[from] InvalidModuleName),
    #[error("absolute import must name a module")]
    EmptyAbsoluteImport,
    #[error("relative import resolved to an empty module name")]
    EmptyRelativeImport,
    #[error("importer is outside project search paths: {0}")]
    ImporterOutsideSearchPaths(String),
    #[error("importer is not a python source file: {0}")]
    ImporterIsNotPythonSource(String),
    #[error("relative import has too many leading dots")]
    TooManyDots,
}

fn python_module_candidates(
    db: &dyn ProjectDb,
    project: Project,
    name: &PythonModuleName,
) -> Vec<PythonModuleCandidate> {
    let relative = name.as_str().replace('.', "/");
    let mut candidates = Vec::new();

    for search_path in project.search_paths(db).iter() {
        let root = search_path.clone();
        let dir = search_path.path().join(&relative);
        let init_file = dir.join("__init__.py");
        if db.path_is_file(&init_file) {
            candidates.push(PythonModuleCandidate::RegularPackage {
                root,
                dir,
                init_file,
            });
            continue;
        }

        let py_file = dir.with_extension("py");
        if db.path_is_file(&py_file) {
            candidates.push(PythonModuleCandidate::FileModule {
                root,
                path: py_file,
            });
            continue;
        }

        if db.path_is_dir(&dir) {
            candidates.push(PythonModuleCandidate::NamespacePortion { root, dir });
        }
    }

    candidates
}

// Salsa tracked-query keys are by-value; `name` is a key, not a borrow.
#[allow(clippy::needless_pass_by_value)]
#[salsa::tracked]
pub fn resolve_module_detail(
    db: &dyn ProjectDb,
    project: Project,
    name: PythonModuleName,
) -> ResolutionDetail {
    project.touch_search_path_roots(db);

    let candidates = python_module_candidates(db, project, &name);
    let selected_root = candidates.iter().find_map(|candidate| match candidate {
        PythonModuleCandidate::RegularPackage { root, .. }
        | PythonModuleCandidate::FileModule { root, .. } => Some(root.clone()),
        PythonModuleCandidate::NamespacePortion { .. } => None,
    });
    let unresolved_reason = if selected_root.is_some() {
        None
    } else if candidates.is_empty() {
        Some(UnresolvedReason::NotFound)
    } else {
        Some(UnresolvedReason::NamespaceOnly)
    };
    let candidates = candidates
        .into_iter()
        .map(|candidate| match candidate {
            PythonModuleCandidate::RegularPackage {
                root, init_file, ..
            } => CandidateLocation {
                root,
                path: init_file,
                kind: CandidateKind::RegularPackage,
            },
            PythonModuleCandidate::FileModule { root, path } => CandidateLocation {
                root,
                path,
                kind: CandidateKind::FileModule,
            },
            PythonModuleCandidate::NamespacePortion { root, dir } => CandidateLocation {
                root,
                path: dir,
                kind: CandidateKind::NamespacePortion,
            },
        })
        .collect();

    ResolutionDetail {
        selected_root,
        candidates,
        unresolved_reason,
    }
}

// Salsa tracked-query keys are by-value; `name` is a key, not a borrow.
#[allow(clippy::needless_pass_by_value)]
#[salsa::tracked]
pub fn resolve_package_dirs(
    db: &dyn ProjectDb,
    project: Project,
    name: PythonModuleName,
) -> PackageDirs {
    project.touch_search_path_roots(db);

    let mut namespace_dirs = Vec::new();
    for candidate in python_module_candidates(db, project, &name) {
        match candidate {
            PythonModuleCandidate::RegularPackage { dir, .. } => {
                return PackageDirs { dirs: vec![dir] };
            }
            PythonModuleCandidate::FileModule { .. } => {
                return PackageDirs { dirs: Vec::new() };
            }
            PythonModuleCandidate::NamespacePortion { dir, .. } => namespace_dirs.push(dir),
        }
    }

    PackageDirs {
        dirs: namespace_dirs,
    }
}

impl PythonModule {
    pub(crate) fn new(name: PythonModuleName, path: Utf8PathBuf, file: File) -> Self {
        Self { name, path, file }
    }

    pub(crate) fn resolve_import(
        db: &dyn ProjectDb,
        project: Project,
        import: PythonImport<'_>,
    ) -> Result<Option<Self>, PythonImportError> {
        let name = if import.level == 0 {
            let module = import
                .module
                .ok_or(PythonImportError::EmptyAbsoluteImport)?;
            PythonModuleName::parse(module)?
        } else {
            let root = project
                .search_paths(db)
                .iter()
                .filter(|search_path| import.importer.starts_with(search_path.path()))
                .max_by_key(|search_path| search_path.path().as_str().len())
                .map(super::search_paths::SearchPath::path)
                .ok_or_else(|| {
                    PythonImportError::ImporterOutsideSearchPaths(import.importer.to_string())
                })?;
            let relative = import.importer.strip_prefix(root).map_err(|_| {
                PythonImportError::ImporterOutsideSearchPaths(import.importer.to_string())
            })?;
            if relative.extension() != Some("py") {
                return Err(PythonImportError::ImporterIsNotPythonSource(
                    import.importer.to_string(),
                ));
            }

            let mut module_parts: Vec<String> = relative
                .parent()
                .ok_or(PythonImportError::EmptyRelativeImport)?
                .components()
                .map(|component| component.as_str().to_string())
                .collect();

            for _ in 1..import.level {
                module_parts.pop().ok_or(PythonImportError::TooManyDots)?;
            }

            if let Some(module) = import.module {
                module_parts.extend(
                    module
                        .split('.')
                        .filter(|part| !part.is_empty())
                        .map(str::to_string),
                );
            }

            if module_parts.is_empty() {
                return Err(PythonImportError::EmptyRelativeImport);
            }

            PythonModuleName::parse(&module_parts.join("."))?
        };

        Ok(Self::resolve(db, project, name))
    }

    #[must_use]
    pub fn name(&self) -> &PythonModuleName {
        &self.name
    }

    #[must_use]
    pub fn path(&self) -> &Utf8Path {
        &self.path
    }

    #[must_use]
    pub fn file(&self) -> File {
        self.file
    }
}

#[salsa::tracked]
impl PythonModule {
    #[salsa::tracked]
    pub fn resolve(db: &dyn ProjectDb, project: Project, name: PythonModuleName) -> Option<Self> {
        project.touch_search_path_roots(db);

        for candidate in python_module_candidates(db, project, &name) {
            let path = match candidate {
                PythonModuleCandidate::RegularPackage { init_file, .. } => init_file,
                PythonModuleCandidate::FileModule { path, .. } => path,
                PythonModuleCandidate::NamespacePortion { .. } => continue,
            };

            let file = db.get_or_create_file(&path);
            return Some(Self::new(name, path, file));
        }

        None
    }

    #[salsa::tracked]
    pub(crate) fn resolve_source_path(
        db: &dyn ProjectDb,
        project: Project,
        source_path: Utf8PathBuf,
    ) -> Option<Self> {
        let search_path = project
            .search_paths(db)
            .iter()
            .filter(|search_path| source_path.starts_with(search_path.path()))
            .max_by_key(|search_path| search_path.path().as_str().len())?;
        let relative = source_path.strip_prefix(search_path.path()).ok()?;
        let name = PythonModuleName::from_relative_source_path(relative).ok()?;
        let module = Self::resolve(db, project, name)?;
        (module.path() == source_path.as_path()).then_some(module)
    }
}

impl fmt::Debug for PythonModule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PythonModule")
            .field("name", &self.name)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl PythonPackage {
    fn new(name: PythonModuleName, dir: Utf8PathBuf, init_file: Option<File>) -> Self {
        Self {
            name,
            dir,
            init_file,
        }
    }

    pub(crate) fn name(&self) -> &PythonModuleName {
        &self.name
    }

    pub(crate) fn dir(&self) -> &Utf8Path {
        &self.dir
    }
}

#[salsa::tracked]
impl PythonPackage {
    #[salsa::tracked]
    pub(crate) fn resolve(
        db: &dyn ProjectDb,
        project: Project,
        name: PythonModuleName,
    ) -> Option<Self> {
        project.touch_search_path_roots(db);

        let relative = name.as_str().replace('.', "/");
        for search_path in project.search_paths(db).iter() {
            let dir = search_path.path().join(&relative);
            if !db.path_is_dir(&dir) {
                continue;
            }

            let init_path = dir.join("__init__.py");
            let init_file = db
                .path_is_file(&init_path)
                .then(|| db.get_or_create_file(&init_path));
            return Some(Self::new(name, dir, init_file));
        }

        None
    }
}

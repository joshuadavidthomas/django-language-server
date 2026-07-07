use std::fmt;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::File;
use djls_source::path_to_file;
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolutionDetail {
    pub selected_root: Option<SearchPath>,
    pub candidates: Vec<CandidateLocation>,
    pub unresolved_reason: Option<UnresolvedReason>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileResolutionDetail {
    pub selected_module: Option<PythonModule>,
    pub derivations: Vec<FileModuleDerivation>,
    pub unresolved_reason: Option<FileUnresolvedReason>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidateLocation {
    pub root: SearchPath,
    pub path: Utf8PathBuf,
    pub kind: CandidateKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileModuleDerivation {
    pub root: SearchPath,
    pub name: PythonModuleName,
    pub resolved_path: Option<Utf8PathBuf>,
    pub status: FileModuleDerivationStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateKind {
    RegularPackage,
    FileModule,
    NamespacePortion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileModuleDerivationStatus {
    RoundTrips,
    Shadowed,
    Unresolved,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnresolvedReason {
    NotFound,
    NamespaceOnly,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileUnresolvedReason {
    Shadowed { winner: Utf8PathBuf },
    NotUnderAnyRoot,
    NoValidDerivation,
    NotFound,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PackageDirs {
    pub dirs: Vec<Utf8PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedPrefix {
    pub module: Option<PythonModule>,
    pub unresolved_tail: Vec<String>,
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

struct FileModuleDerivationResult {
    root: SearchPath,
    name: PythonModuleName,
    resolved_module: Option<PythonModule>,
    status: FileModuleDerivationStatus,
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

fn file_module_names<'a>(
    db: &'a dyn ProjectDb,
    project: Project,
    source_path: &'a Utf8Path,
) -> impl Iterator<Item = (&'a SearchPath, PythonModuleName)> + 'a {
    project
        .search_paths(db)
        .iter()
        .filter_map(move |search_path| {
            let relative = source_path.strip_prefix(search_path.path()).ok()?;
            let name = PythonModuleName::from_relative_source_path(relative).ok()?;
            Some((search_path, name))
        })
}

fn file_module_derivation_result(
    db: &dyn ProjectDb,
    project: Project,
    source_path: &Utf8Path,
    search_path: &SearchPath,
    name: PythonModuleName,
) -> FileModuleDerivationResult {
    let resolved_module = PythonModule::resolve(db, project, name.clone());
    let status = match resolved_module.as_ref().map(PythonModule::path) {
        Some(resolved_path) if resolved_path == source_path => {
            FileModuleDerivationStatus::RoundTrips
        }
        Some(_) => FileModuleDerivationStatus::Shadowed,
        None => FileModuleDerivationStatus::Unresolved,
    };

    FileModuleDerivationResult {
        root: search_path.clone(),
        name,
        resolved_module,
        status,
    }
}

fn selected_file_module(selected: Option<&FileModuleDerivationResult>) -> Option<PythonModule> {
    let selected = selected?;
    if selected.status == FileModuleDerivationStatus::RoundTrips {
        selected.resolved_module.clone()
    } else {
        None
    }
}

pub fn resolve_prefix(db: &dyn ProjectDb, project: Project, dotted_path: &str) -> ResolvedPrefix {
    let segments: Vec<&str> = dotted_path.split('.').collect();

    for prefix_len in (1..=segments.len()).rev() {
        let prefix = segments[..prefix_len].join(".");
        let Ok(name) = PythonModuleName::parse(&prefix) else {
            continue;
        };
        let Some(module) = PythonModule::resolve(db, project, name) else {
            continue;
        };

        return ResolvedPrefix {
            module: Some(module),
            unresolved_tail: segments[prefix_len..]
                .iter()
                .map(|segment| (*segment).to_string())
                .collect(),
        };
    }

    ResolvedPrefix {
        module: None,
        unresolved_tail: segments
            .iter()
            .map(|segment| (*segment).to_string())
            .collect(),
    }
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

// Salsa tracked-query keys are by-value; `source_path` is a key, not a borrow.
#[allow(clippy::needless_pass_by_value)]
#[salsa::tracked]
pub fn file_to_module(
    db: &dyn ProjectDb,
    project: Project,
    source_path: Utf8PathBuf,
) -> Option<PythonModule> {
    project.touch_search_path_roots(db);

    let selected = file_module_names(db, project, source_path.as_path())
        .next()
        .map(|(root, name)| {
            file_module_derivation_result(db, project, source_path.as_path(), root, name)
        });
    selected_file_module(selected.as_ref())
}

// Salsa tracked-query keys are by-value; `source_path` is a key, not a borrow.
#[allow(clippy::needless_pass_by_value)]
#[salsa::tracked]
pub fn file_to_module_detail(
    db: &dyn ProjectDb,
    project: Project,
    source_path: Utf8PathBuf,
) -> FileResolutionDetail {
    project.touch_search_path_roots(db);

    let results = file_module_names(db, project, source_path.as_path())
        .map(|(root, name)| {
            file_module_derivation_result(db, project, source_path.as_path(), root, name)
        })
        .collect::<Vec<_>>();
    let selected_module = selected_file_module(results.first());
    let unresolved_reason = if selected_module.is_some() {
        None
    } else if let Some(selected) = results.first() {
        match selected.resolved_module.as_ref().map(PythonModule::path) {
            Some(winner) => Some(FileUnresolvedReason::Shadowed {
                winner: winner.to_path_buf(),
            }),
            None => Some(FileUnresolvedReason::NotFound),
        }
    } else if project
        .search_paths(db)
        .iter()
        .any(|search_path| source_path.strip_prefix(search_path.path()).is_ok())
    {
        Some(FileUnresolvedReason::NoValidDerivation)
    } else {
        Some(FileUnresolvedReason::NotUnderAnyRoot)
    };
    let derivations = results
        .iter()
        .map(|derivation| FileModuleDerivation {
            root: derivation.root.clone(),
            name: derivation.name.clone(),
            resolved_path: derivation
                .resolved_module
                .as_ref()
                .map(|module| module.path().to_path_buf()),
            status: derivation.status,
        })
        .collect();

    FileResolutionDetail {
        selected_module,
        derivations,
        unresolved_reason,
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

            let file = path_to_file(db, &path).ok()?;
            return Some(Self::new(name, path, file));
        }

        None
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

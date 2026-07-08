use std::fmt;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::File;
use djls_source::FileError;
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct CandidateDirectory {
    root: SearchPath,
    dir: Utf8PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ModuleResolution {
    RegularPackage {
        root: SearchPath,
        dir: Utf8PathBuf,
        init_file: Utf8PathBuf,
        file: File,
    },
    FileModule {
        root: SearchPath,
        path: Utf8PathBuf,
        file: File,
    },
    NamespaceOnly {
        portions: Vec<CandidateDirectory>,
    },
    NotFound,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ResolvedComponent {
    RegularPackage {
        root: SearchPath,
        dir: Utf8PathBuf,
        init_file: Utf8PathBuf,
        file: File,
    },
    FileModule {
        root: SearchPath,
        path: Utf8PathBuf,
        file: File,
    },
    NamespacePortion(CandidateDirectory),
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

/// Source text plus the Python module file identity that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonModuleSource {
    source: String,
    file: File,
    path: Utf8PathBuf,
}

impl PythonModuleSource {
    pub(crate) fn new(file: File, path: Utf8PathBuf, source: String) -> Self {
        Self { source, file, path }
    }

    pub(crate) fn source(&self) -> &str {
        &self.source
    }

    pub(crate) fn file(&self) -> File {
        self.file
    }

    pub(crate) fn path(&self) -> &Utf8Path {
        &self.path
    }
}

/// Import-following seam used by Python source extractors.
pub(crate) trait PythonImportSourceResolver {
    fn resolve_star_import(&mut self, import: PythonImport<'_>) -> Option<PythonModuleSource>;

    fn resolve_named_import(&mut self, import: PythonImport<'_>) -> Option<PythonModuleSource>;
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

fn python_module_flat_candidates(
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
        if db.path_is_file(&init_file) && path_case_matches(db, &init_file, search_path.path()) {
            candidates.push(PythonModuleCandidate::RegularPackage {
                root,
                dir,
                init_file,
            });
            continue;
        }

        let py_file = dir.with_extension("py");
        if db.path_is_file(&py_file) && path_case_matches(db, &py_file, search_path.path()) {
            candidates.push(PythonModuleCandidate::FileModule {
                root,
                path: py_file,
            });
            continue;
        }

        if db.path_is_dir(&dir) && path_case_matches(db, &dir, search_path.path()) {
            candidates.push(PythonModuleCandidate::NamespacePortion { root, dir });
        }
    }

    candidates
}

fn path_case_matches(db: &dyn ProjectDb, path: &Utf8Path, prefix: &Utf8Path) -> bool {
    let fs = db.file_system();
    fs.case_sensitivity().is_case_sensitive() || fs.path_exists_case_sensitive(path, prefix)
}

fn resolve_component(
    db: &dyn ProjectDb,
    candidate: &CandidateDirectory,
    component: &str,
) -> Option<ResolvedComponent> {
    let dir = candidate.dir.join(component);
    let dir_status = path_to_file(db, &dir);
    let init_file = dir.join("__init__.py");
    if matches!(dir_status, Err(FileError::IsADirectory))
        && let Ok(file) = path_to_file(db, &init_file)
    {
        return Some(ResolvedComponent::RegularPackage {
            root: candidate.root.clone(),
            dir,
            init_file,
            file,
        });
    }

    let py_file = dir.with_extension("py");
    if let Ok(file) = path_to_file(db, &py_file) {
        return Some(ResolvedComponent::FileModule {
            root: candidate.root.clone(),
            path: py_file,
            file,
        });
    }

    if matches!(dir_status, Err(FileError::IsADirectory)) {
        return Some(ResolvedComponent::NamespacePortion(CandidateDirectory {
            root: candidate.root.clone(),
            dir,
        }));
    }

    None
}

fn resolve_name(db: &dyn ProjectDb, project: Project, name: &PythonModuleName) -> ModuleResolution {
    let mut candidate_dirs = project
        .search_paths(db)
        .iter()
        .map(|search_path| CandidateDirectory {
            root: search_path.clone(),
            dir: search_path.path().to_path_buf(),
        })
        .collect::<Vec<_>>();
    let components = name.as_str().split('.').collect::<Vec<_>>();

    for (index, component) in components.iter().enumerate() {
        let is_last = index + 1 == components.len();
        let mut portions = Vec::new();
        let mut next_dirs = None;

        for candidate in &candidate_dirs {
            match resolve_component(db, candidate, component) {
                Some(ResolvedComponent::RegularPackage {
                    root,
                    dir,
                    init_file,
                    file,
                }) => {
                    if is_last {
                        return ModuleResolution::RegularPackage {
                            root,
                            dir,
                            init_file,
                            file,
                        };
                    }
                    next_dirs = Some(vec![CandidateDirectory { root, dir }]);
                    break;
                }
                Some(ResolvedComponent::FileModule { root, path, file }) => {
                    if is_last {
                        return ModuleResolution::FileModule { root, path, file };
                    }
                    return ModuleResolution::NotFound;
                }
                Some(ResolvedComponent::NamespacePortion(portion)) => portions.push(portion),
                None => {}
            }
        }

        candidate_dirs = match next_dirs {
            Some(dirs) => dirs,
            None if portions.is_empty() => return ModuleResolution::NotFound,
            None if is_last => return ModuleResolution::NamespaceOnly { portions },
            None => portions,
        };
    }

    ModuleResolution::NotFound
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

    let resolution = resolve_name(db, project, &name);
    let selected_root = match &resolution {
        ModuleResolution::RegularPackage { root, .. }
        | ModuleResolution::FileModule { root, .. } => Some(root.clone()),
        ModuleResolution::NamespaceOnly { .. } | ModuleResolution::NotFound => None,
    };
    let unresolved_reason = match &resolution {
        ModuleResolution::RegularPackage { .. } | ModuleResolution::FileModule { .. } => None,
        ModuleResolution::NamespaceOnly { .. } => Some(UnresolvedReason::NamespaceOnly),
        ModuleResolution::NotFound => Some(UnresolvedReason::NotFound),
    };
    let candidates = python_module_flat_candidates(db, project, &name);
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
    match resolve_name(db, project, &name) {
        ModuleResolution::RegularPackage { dir, .. } => PackageDirs { dirs: vec![dir] },
        ModuleResolution::FileModule { .. } | ModuleResolution::NotFound => {
            PackageDirs { dirs: Vec::new() }
        }
        ModuleResolution::NamespaceOnly { portions } => PackageDirs {
            dirs: portions.into_iter().map(|portion| portion.dir).collect(),
        },
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
        match resolve_name(db, project, &name) {
            ModuleResolution::RegularPackage {
                init_file, file, ..
            } => Some(Self::new(name, init_file, file)),
            ModuleResolution::FileModule { path, file, .. } => Some(Self::new(name, path, file)),
            ModuleResolution::NamespaceOnly { .. } | ModuleResolution::NotFound => None,
        }
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

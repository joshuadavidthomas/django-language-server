use std::cmp::Ordering;
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
use crate::python::evaluation::StructuralOrd;
use crate::python::search_paths::SearchPath;

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct PythonModule {
    name: PythonModuleName,
    package: Option<PythonModuleName>,
    path: Utf8PathBuf,
    file: File,
    search_path: SearchPath,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileModuleResolution {
    Candidates {
        first: FileModuleCandidate,
        rest: Vec<FileModuleCandidate>,
    },
    OutsideSearchPaths,
    InvalidModuleName,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileModuleCandidate {
    Resolved(PythonModule),
    Shadowed {
        root: SearchPath,
        name: PythonModuleName,
        winner: PythonModule,
    },
    NotFound {
        root: SearchPath,
        name: PythonModuleName,
    },
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
struct CandidateDirectory {
    root: SearchPath,
    dir: Utf8PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ModuleLookupResult {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PythonImportRequest<'a> {
    pub(crate) level: u32,
    pub(crate) module: Option<&'a str>,
    pub(crate) importer: &'a PythonModule,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub(crate) enum PythonImportResolutionError {
    #[error(transparent)]
    Invalid(#[from] PythonImportError),
    #[error("Python module `{0}` was not found")]
    NotFound(PythonModuleName),
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub(crate) enum PythonImportError {
    #[error(transparent)]
    InvalidModuleName(#[from] InvalidModuleName),
    #[error("absolute import must name a module")]
    EmptyAbsoluteImport,
    #[error("relative import has too many leading dots")]
    TooManyDots,
}

impl CandidateDirectory {
    fn resolve_component(&self, db: &dyn ProjectDb, component: &str) -> Option<ResolvedComponent> {
        let dir = self.dir.join(component);
        let dir_status = path_to_file(db, &dir);
        let init_file = dir.join("__init__.py");
        if matches!(dir_status, Err(FileError::IsADirectory))
            && let Ok(file) = path_to_file(db, &init_file)
        {
            return Some(ResolvedComponent::RegularPackage {
                root: self.root.clone(),
                dir,
                init_file,
                file,
            });
        }

        let py_file = dir.with_extension("py");
        if let Ok(file) = path_to_file(db, &py_file) {
            return Some(ResolvedComponent::FileModule {
                root: self.root.clone(),
                path: py_file,
                file,
            });
        }

        if matches!(dir_status, Err(FileError::IsADirectory)) {
            return Some(ResolvedComponent::NamespacePortion(Self {
                root: self.root.clone(),
                dir,
            }));
        }

        None
    }
}

fn resolve_name(
    db: &dyn ProjectDb,
    project: Project,
    name: &PythonModuleName,
) -> ModuleLookupResult {
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
            match candidate.resolve_component(db, component) {
                Some(ResolvedComponent::RegularPackage {
                    root,
                    dir,
                    init_file,
                    file,
                }) => {
                    if is_last {
                        return ModuleLookupResult::RegularPackage {
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
                        return ModuleLookupResult::FileModule { root, path, file };
                    }
                    return ModuleLookupResult::NotFound;
                }
                Some(ResolvedComponent::NamespacePortion(portion)) => portions.push(portion),
                None => {}
            }
        }

        candidate_dirs = match next_dirs {
            Some(dirs) => dirs,
            None if portions.is_empty() => return ModuleLookupResult::NotFound,
            None if is_last => return ModuleLookupResult::NamespaceOnly { portions },
            None => portions,
        };
    }

    ModuleLookupResult::NotFound
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

fn file_module_candidate(
    db: &dyn ProjectDb,
    project: Project,
    source_path: &Utf8Path,
    search_path: &SearchPath,
    name: PythonModuleName,
) -> FileModuleCandidate {
    match PythonModule::resolve(db, project, name.clone()) {
        Some(module) if module.path() == source_path => FileModuleCandidate::Resolved(module),
        Some(winner) => FileModuleCandidate::Shadowed {
            root: search_path.clone(),
            name,
            winner,
        },
        None => FileModuleCandidate::NotFound {
            root: search_path.clone(),
            name,
        },
    }
}

/// Construct the dotted source-module name a `from [.]module import ...`
/// clause targets, given the importing file's containing `package`.
///
/// This is the single owner of relative-import name construction, shared by
/// import evaluation ([`PythonModule::resolve_import`]) and Model alias
/// resolution. It never infers package semantics from a dotted name alone: the
/// caller supplies the package identity explicitly. Returns `None` when a
/// relative level climbs past the package root or nothing remains.
pub(crate) fn relative_import_source(
    package: Option<&PythonModuleName>,
    level: u32,
    module: Option<&str>,
) -> Option<String> {
    if level == 0 {
        return module.map(str::to_string);
    }

    let mut parts: Vec<&str> = package
        .map(|package| package.as_str().split('.').collect())
        .unwrap_or_default();
    if level as usize > parts.len() {
        return None;
    }
    for _ in 1..level {
        parts.pop();
    }

    if let Some(module) = module {
        parts.extend(module.split('.').filter(|part| !part.is_empty()));
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("."))
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
pub fn resolve_package_dirs(
    db: &dyn ProjectDb,
    project: Project,
    name: PythonModuleName,
) -> PackageDirs {
    match resolve_name(db, project, &name) {
        ModuleLookupResult::RegularPackage { dir, .. } => PackageDirs { dirs: vec![dir] },
        ModuleLookupResult::FileModule { .. } | ModuleLookupResult::NotFound => {
            PackageDirs { dirs: Vec::new() }
        }
        ModuleLookupResult::NamespaceOnly { portions } => PackageDirs {
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
    let candidate = file_module_names(db, project, source_path.as_path())
        .next()
        .map(|(root, name)| file_module_candidate(db, project, source_path.as_path(), root, name));

    match candidate {
        Some(FileModuleCandidate::Resolved(module)) => Some(module),
        Some(
            FileModuleCandidate::Shadowed {
                root: _,
                name: _,
                winner: _,
            }
            | FileModuleCandidate::NotFound { root: _, name: _ },
        )
        | None => None,
    }
}

// Salsa tracked-query keys are by-value; `source_path` is a key, not a borrow.
#[allow(clippy::needless_pass_by_value)]
#[salsa::tracked(returns(ref))]
pub fn file_to_module_resolution(
    db: &dyn ProjectDb,
    project: Project,
    source_path: Utf8PathBuf,
) -> FileModuleResolution {
    project.touch_search_path_roots(db);

    let mut candidates = file_module_names(db, project, source_path.as_path())
        .map(|(root, name)| file_module_candidate(db, project, source_path.as_path(), root, name));
    let Some(first) = candidates.next() else {
        return if project
            .search_paths(db)
            .iter()
            .any(|search_path| source_path.starts_with(search_path.path()))
        {
            FileModuleResolution::InvalidModuleName
        } else {
            FileModuleResolution::OutsideSearchPaths
        };
    };

    FileModuleResolution::Candidates {
        first,
        rest: candidates.collect(),
    }
}

impl StructuralOrd for PythonModule {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        self.name
            .cmp(&other.name)
            .then_with(|| self.package.cmp(&other.package))
            .then_with(|| self.path.cmp(&other.path))
            .then_with(|| self.file.structural_cmp(&other.file))
            .then_with(|| self.search_path.structural_cmp(&other.search_path))
    }
}

impl PythonModule {
    fn regular_package(
        name: PythonModuleName,
        path: Utf8PathBuf,
        file: File,
        search_path: SearchPath,
    ) -> Self {
        Self {
            package: Some(name.clone()),
            name,
            path,
            file,
            search_path,
        }
    }

    pub(crate) fn file_module(
        name: PythonModuleName,
        path: Utf8PathBuf,
        file: File,
        search_path: SearchPath,
    ) -> Self {
        Self {
            package: name.parent(),
            name,
            path,
            file,
            search_path,
        }
    }

    pub(crate) fn resolve_import(
        db: &dyn ProjectDb,
        project: Project,
        import: PythonImportRequest<'_>,
    ) -> Result<PythonModule, PythonImportResolutionError> {
        let name = if import.level == 0 {
            let module = import
                .module
                .ok_or(PythonImportError::EmptyAbsoluteImport)?;
            PythonModuleName::parse(module).map_err(PythonImportError::from)?
        } else {
            let source = relative_import_source(
                import.importer.package.as_ref(),
                import.level,
                import.module,
            )
            .ok_or(PythonImportError::TooManyDots)?;
            PythonModuleName::parse(&source).map_err(PythonImportError::from)?
        };

        Self::resolve(db, project, name.clone()).ok_or(PythonImportResolutionError::NotFound(name))
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

    #[must_use]
    pub fn search_path(&self) -> &SearchPath {
        &self.search_path
    }
}

#[salsa::tracked]
impl PythonModule {
    #[salsa::tracked]
    pub fn resolve(db: &dyn ProjectDb, project: Project, name: PythonModuleName) -> Option<Self> {
        match resolve_name(db, project, &name) {
            ModuleLookupResult::RegularPackage {
                root,
                init_file,
                file,
                ..
            } => Some(Self::regular_package(name, init_file, file, root)),
            ModuleLookupResult::FileModule { root, path, file } => {
                Some(Self::file_module(name, path, file, root))
            }
            ModuleLookupResult::NamespaceOnly { .. } | ModuleLookupResult::NotFound => None,
        }
    }
}

impl StructuralOrd for PythonImportError {
    fn structural_cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::EmptyAbsoluteImport, Self::EmptyAbsoluteImport)
            | (Self::TooManyDots, Self::TooManyDots) => Ordering::Equal,
            (Self::InvalidModuleName(left), Self::InvalidModuleName(right)) => {
                left.structural_cmp(right)
            }
            (Self::EmptyAbsoluteImport, Self::InvalidModuleName(_) | Self::TooManyDots)
            | (Self::InvalidModuleName(_), Self::TooManyDots) => Ordering::Less,
            (Self::InvalidModuleName(_) | Self::TooManyDots, Self::EmptyAbsoluteImport)
            | (Self::TooManyDots, Self::InvalidModuleName(_)) => Ordering::Greater,
        }
    }
}

impl fmt::Debug for PythonModule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PythonModule")
            .field("name", &self.name)
            .field("package", &self.package)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use salsa::plumbing::FromId;
    use salsa::plumbing::Id;

    use super::*;

    fn module_parts(name: &str) -> (PythonModuleName, Utf8PathBuf, File, SearchPath) {
        (
            PythonModuleName::parse(name).unwrap(),
            Utf8PathBuf::from(format!("/project/{}.py", name.replace('.', "/"))),
            File::from_id(Id::from_bits(1)),
            SearchPath::FirstParty(Utf8PathBuf::from("/project")),
        )
    }

    #[test]
    fn typed_module_order_compares_every_equality_bearing_field() {
        let base = PythonModule {
            name: PythonModuleName::parse("pkg.module").unwrap(),
            package: Some(PythonModuleName::parse("pkg").unwrap()),
            path: Utf8PathBuf::from("/project/pkg/module.py"),
            file: File::from_id(Id::from_bits(15)),
            search_path: SearchPath::FirstParty(Utf8PathBuf::from("/project")),
        };
        let unequal = [
            PythonModule {
                name: PythonModuleName::parse("pkg.other").unwrap(),
                ..base.clone()
            },
            PythonModule {
                package: None,
                ..base.clone()
            },
            PythonModule {
                path: Utf8PathBuf::from("/project/pkg/other.py"),
                ..base.clone()
            },
            PythonModule {
                file: File::from_id(Id::from_bits(16)),
                ..base.clone()
            },
            PythonModule {
                search_path: SearchPath::Extra(Utf8PathBuf::from("/project")),
                ..base.clone()
            },
        ];

        assert_eq!(base.structural_cmp(&base), Ordering::Equal);
        for other in &unequal {
            assert_ne!(base.structural_cmp(other), Ordering::Equal);
            assert_eq!(
                base.structural_cmp(other),
                other.structural_cmp(&base).reverse()
            );
        }
    }

    #[test]
    fn python_module_package_identity_is_derived_by_semantic_kind() {
        let (name, path, file, search_path) = module_parts("pkg");
        let package = PythonModule::regular_package(name, path, file, search_path);
        assert_eq!(
            package.package.as_ref().map(PythonModuleName::as_str),
            Some("pkg")
        );

        for (name, expected_package) in [
            ("top_level", None),
            ("pkg.settings", Some("pkg")),
            ("pkg.__init__", Some("pkg")),
            ("app.templatetags.tags", Some("app.templatetags")),
        ] {
            let (name, path, file, search_path) = module_parts(name);
            let module = PythonModule::file_module(name, path, file, search_path);
            assert_eq!(
                module.package.as_ref().map(PythonModuleName::as_str),
                expected_package
            );
        }
    }
}

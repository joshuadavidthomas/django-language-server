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
pub struct PythonSourceModule {
    name: PythonModuleName,
    package: Option<PythonModuleName>,
    path: Utf8PathBuf,
    file: File,
    search_path: SearchPath,
}

/// One search-path portion that contributes to a namespace package.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct NamespacePortion {
    root: SearchPath,
    dir: Utf8PathBuf,
}

impl NamespacePortion {
    pub(crate) fn new(root: SearchPath, dir: Utf8PathBuf) -> Self {
        Self { root, dir }
    }

    pub(crate) fn root(&self) -> &SearchPath {
        &self.root
    }

    pub(crate) fn dir(&self) -> &Utf8PathBuf {
        &self.dir
    }
}

/// The identity of a namespace package with no source body of its own.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct PythonNamespacePackage {
    name: PythonModuleName,
    portions: Vec<NamespacePortion>,
}

impl PythonNamespacePackage {
    pub(crate) fn new(name: PythonModuleName, portions: Vec<NamespacePortion>) -> Self {
        Self { name, portions }
    }

    pub(crate) fn name(&self) -> &PythonModuleName {
        &self.name
    }

    pub(crate) fn portions(&self) -> &[NamespacePortion] {
        &self.portions
    }
}

/// The source-or-namespace identity carried by a Python module value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PythonModule {
    Source(PythonSourceModule),
    Namespace(PythonNamespacePackage),
}

impl PythonModule {
    pub(crate) fn name(&self) -> &PythonModuleName {
        match self {
            Self::Source(module) => module.name(),
            Self::Namespace(package) => package.name(),
        }
    }

    pub(crate) fn is_package(&self) -> bool {
        match self {
            Self::Source(module) => module.is_package(),
            Self::Namespace(_) => true,
        }
    }
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
    Resolved(PythonSourceModule),
    Shadowed {
        root: SearchPath,
        name: PythonModuleName,
        winner: PythonSourceModule,
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
    pub module: Option<PythonSourceModule>,
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
    pub(crate) importer: &'a PythonSourceModule,
}

/// A contiguous, ordered root-to-leaf prefix of source/namespace components.
///
/// The empty chain is only ever produced as the prefix of a root failure. The
/// resolver enforces contiguity: every component is reachable from the previous
/// one, so a chain never skips a package boundary.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ResolvedImportChain {
    components: Vec<PythonModule>,
}

impl ResolvedImportChain {
    pub(crate) fn into_components(self) -> Vec<PythonModule> {
        self.components
    }
}

/// The outcome of resolving a full dotted import target into a component chain.
///
/// A successful resolution owns a complete contiguous chain; a failure owns the
/// resolved prefix (possibly empty) plus the typed reason the next component
/// could not be resolved. Earlier prefix components remain available so an
/// importer can preserve their effects even when a later component fails.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PythonImportChainResolution {
    Resolved(ResolvedImportChain),
    Failed {
        prefix: ResolvedImportChain,
        failure: PythonImportChainFailure,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub(crate) enum PythonImportChainFailure {
    #[error(transparent)]
    Invalid(#[from] PythonImportNameError),
    #[error("Python module `{0}` was not found")]
    NotFound(PythonModuleName),
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub(crate) enum PythonImportNameError {
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

/// Resolve a fully-qualified dotted name to its leaf lookup result.
///
/// This is a thin projection of the single chain traversal in
/// [`resolve_chain_from_name`]: it keeps only the leaf component that non-import
/// callers ([`PythonSourceModule::resolve`], [`resolve_package_dirs`]) need. There is
/// no second component walker; leaf and chain resolution share one traversal.
fn resolve_name(
    db: &dyn ProjectDb,
    project: Project,
    name: &PythonModuleName,
) -> ModuleLookupResult {
    let chain = match resolve_chain_from_name(db, project, name) {
        PythonImportChainResolution::Resolved(chain) => chain,
        PythonImportChainResolution::Failed { .. } => return ModuleLookupResult::NotFound,
    };
    match chain.into_components().pop() {
        Some(PythonModule::Source(module)) => {
            // A regular package's derived package identity is its own name; a
            // file module's is its parent. This distinguishes a genuine
            // `pkg/__init__.py` package from an explicit `pkg.__init__` file
            // alias, which a path-suffix check cannot.
            let is_regular_package = module.is_package();
            let root = module.search_path().clone();
            let path = module.path().to_path_buf();
            let file = module.file();
            if is_regular_package {
                let dir = path
                    .parent()
                    .map_or_else(|| path.clone(), Utf8Path::to_path_buf);
                ModuleLookupResult::RegularPackage {
                    root,
                    dir,
                    init_file: path,
                    file,
                }
            } else {
                ModuleLookupResult::FileModule { root, path, file }
            }
        }
        Some(PythonModule::Namespace(package)) => ModuleLookupResult::NamespaceOnly {
            portions: package
                .portions()
                .iter()
                .map(|portion| CandidateDirectory {
                    root: portion.root().clone(),
                    dir: portion.dir().clone(),
                })
                .collect(),
        },
        None => ModuleLookupResult::NotFound,
    }
}

/// Resolve a fully-qualified dotted name into a contiguous root-to-leaf
/// component chain, or a typed failure carrying the resolved prefix.
///
/// This deepens the same first-match/search-path traversal as [`resolve_name`]:
/// it preserves regular-package priority over file modules and namespace
/// portions, honors search-path order, and records package-init identities for
/// intermediate packages. Unlike [`resolve_name`], it captures every
/// intermediate component so an import can evaluate and attach parents.
fn resolve_chain_from_name(
    db: &dyn ProjectDb,
    project: Project,
    name: &PythonModuleName,
) -> PythonImportChainResolution {
    let mut candidate_dirs = project
        .search_paths(db)
        .iter()
        .map(|search_path| CandidateDirectory {
            root: search_path.clone(),
            dir: search_path.path().to_path_buf(),
        })
        .collect::<Vec<_>>();
    let components = name.as_str().split('.').collect::<Vec<_>>();
    let component_names = name.prefixes();
    let mut resolved: Vec<PythonModule> = Vec::new();

    for (index, (component, component_name)) in components.iter().zip(component_names).enumerate() {
        let is_last = index + 1 == components.len();

        let mut portions = Vec::new();
        let mut resolved_source: Option<(PythonSourceModule, Option<CandidateDirectory>)> = None;

        for candidate in &candidate_dirs {
            match candidate.resolve_component(db, component) {
                Some(ResolvedComponent::RegularPackage {
                    root,
                    dir,
                    init_file,
                    file,
                }) => {
                    let module = PythonSourceModule::regular_package(
                        component_name.clone(),
                        init_file,
                        file,
                        root.clone(),
                    );
                    resolved_source = Some((module, Some(CandidateDirectory { root, dir })));
                    break;
                }
                Some(ResolvedComponent::FileModule { root, path, file }) => {
                    // A file module wins this component but has no children. It
                    // still belongs in the resolved prefix (its effects load
                    // even when the requested name extends past it); the
                    // childless-tail failure is applied after it is pushed.
                    let module =
                        PythonSourceModule::file_module(component_name.clone(), path, file, root);
                    resolved_source = Some((module, None));
                    break;
                }
                Some(ResolvedComponent::NamespacePortion(portion)) => portions.push(portion),
                None => {}
            }
        }

        match resolved_source {
            Some((module, next_dir)) => {
                resolved.push(PythonModule::Source(module));
                match next_dir {
                    // A regular package narrows the search to its own directory.
                    Some(dir) => candidate_dirs = vec![dir],
                    // A file module has no children. If the requested name still
                    // has further components, it is unresolvable past this
                    // winning file module, which now survives in the prefix.
                    None if !is_last => {
                        return PythonImportChainResolution::Failed {
                            prefix: ResolvedImportChain {
                                components: resolved,
                            },
                            failure: PythonImportChainFailure::NotFound(name.clone()),
                        };
                    }
                    None => {}
                }
            }
            None if portions.is_empty() => {
                return PythonImportChainResolution::Failed {
                    prefix: ResolvedImportChain {
                        components: resolved,
                    },
                    failure: PythonImportChainFailure::NotFound(name.clone()),
                };
            }
            None => {
                let namespace_portions = portions
                    .iter()
                    .map(|portion| NamespacePortion::new(portion.root.clone(), portion.dir.clone()))
                    .collect();
                resolved.push(PythonModule::Namespace(PythonNamespacePackage::new(
                    component_name,
                    namespace_portions,
                )));
                candidate_dirs = portions;
            }
        }
    }

    PythonImportChainResolution::Resolved(ResolvedImportChain {
        components: resolved,
    })
}

/// Build the fully-qualified module name a `[from] import` operation targets,
/// resolving relative levels against the importer's package. This is the single
/// owner of import name construction shared by chain resolution.
fn import_module_name(
    import: PythonImportRequest<'_>,
) -> Result<PythonModuleName, PythonImportNameError> {
    if import.level == 0 {
        let module = import
            .module
            .ok_or(PythonImportNameError::EmptyAbsoluteImport)?;
        PythonModuleName::parse(module).map_err(PythonImportNameError::from)
    } else {
        let source = relative_import_source(
            import.importer.package.as_ref(),
            import.level,
            import.module,
        )
        .ok_or(PythonImportNameError::TooManyDots)?;
        PythonModuleName::parse(&source).map_err(PythonImportNameError::from)
    }
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
    match PythonSourceModule::resolve(db, project, name.clone()) {
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
/// import evaluation ([`PythonSourceModule::resolve_import_chain`]) and Model alias
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
        let Some(module) = PythonSourceModule::resolve(db, project, name) else {
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
#[salsa::tracked(returns(clone))]
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
#[salsa::tracked(returns(clone))]
pub fn file_to_module(
    db: &dyn ProjectDb,
    project: Project,
    source_path: Utf8PathBuf,
) -> Option<PythonSourceModule> {
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

impl PythonSourceModule {
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

    /// Resolve an import operation into a contiguous root-to-leaf component
    /// chain. Name-construction failures (empty absolute import, too many
    /// relative dots, invalid module name) are typed [`PythonImportNameError`]s that
    /// yield no chain; an unresolvable component is a
    /// [`PythonImportChainResolution::Failed`] carrying the resolved prefix.
    pub(crate) fn resolve_import_chain(
        db: &dyn ProjectDb,
        project: Project,
        import: PythonImportRequest<'_>,
    ) -> Result<(PythonModuleName, PythonImportChainResolution), PythonImportNameError> {
        let name = import_module_name(import)?;
        let resolution = resolve_chain_from_name(db, project, &name);
        Ok((name, resolution))
    }

    #[must_use]
    pub fn name(&self) -> &PythonModuleName {
        &self.name
    }

    /// Whether this identity is a regular package rather than a file module.
    /// Package policy belongs to the resolved identity; callers must not infer
    /// it from `__init__.py` path spelling.
    #[must_use]
    fn is_package(&self) -> bool {
        self.package.as_ref() == Some(&self.name)
    }

    pub(crate) fn package(&self) -> Option<&PythonModuleName> {
        self.package.as_ref()
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
impl PythonSourceModule {
    #[salsa::tracked(returns(clone))]
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

impl StructuralOrd for PythonImportNameError {
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

impl fmt::Debug for PythonSourceModule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PythonSourceModule")
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

    #[test]
    fn resolved_import_chain_exposes_ordered_components_and_empty_prefix() {
        let (name, path, file, search_path) = module_parts("pkg");
        let package = PythonSourceModule::regular_package(name, path, file, search_path);
        let (leaf_name, leaf_path, leaf_file, leaf_search) = module_parts("pkg.sub");
        let leaf = PythonSourceModule::file_module(leaf_name, leaf_path, leaf_file, leaf_search);

        let chain = ResolvedImportChain {
            components: vec![
                PythonModule::Namespace(PythonNamespacePackage::new(
                    PythonModuleName::parse("pkg")
                        .expect("test Python module name should be valid"),
                    Vec::new(),
                )),
                PythonModule::Source(package),
                PythonModule::Source(leaf),
            ],
        };
        assert!(!chain.components.is_empty());
        let names: Vec<_> = chain
            .components
            .iter()
            .map(|component| component.name().as_str().to_string())
            .collect();
        assert_eq!(names, ["pkg", "pkg", "pkg.sub"]);

        // A root failure carries an empty resolved prefix.
        let root_failure = PythonImportChainResolution::Failed {
            prefix: ResolvedImportChain::default(),
            failure: PythonImportChainFailure::NotFound(
                PythonModuleName::parse("missing")
                    .expect("test Python module name should be valid"),
            ),
        };
        assert!(matches!(
            root_failure,
            PythonImportChainResolution::Failed { prefix, .. } if prefix.components.is_empty()
        ));
    }

    fn module_parts(name: &str) -> (PythonModuleName, Utf8PathBuf, File, SearchPath) {
        (
            PythonModuleName::parse(name).expect("test Python module name should be valid"),
            Utf8PathBuf::from(format!("/project/{}.py", name.replace('.', "/"))),
            File::from_id(Id::from_bits(1)),
            SearchPath::FirstParty(Utf8PathBuf::from("/project")),
        )
    }

    #[test]
    fn typed_module_order_compares_every_equality_bearing_field() {
        let base = PythonSourceModule {
            name: PythonModuleName::parse("pkg.module")
                .expect("test Python module name should be valid"),
            package: Some(
                PythonModuleName::parse("pkg").expect("test Python module name should be valid"),
            ),
            path: Utf8PathBuf::from("/project/pkg/module.py"),
            file: File::from_id(Id::from_bits(15)),
            search_path: SearchPath::FirstParty(Utf8PathBuf::from("/project")),
        };
        let unequal = [
            PythonSourceModule {
                name: PythonModuleName::parse("pkg.other")
                    .expect("test Python module name should be valid"),
                ..base.clone()
            },
            PythonSourceModule {
                package: None,
                ..base.clone()
            },
            PythonSourceModule {
                path: Utf8PathBuf::from("/project/pkg/other.py"),
                ..base.clone()
            },
            PythonSourceModule {
                file: File::from_id(Id::from_bits(16)),
                ..base.clone()
            },
            PythonSourceModule {
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
        let package = PythonSourceModule::regular_package(name, path, file, search_path);
        assert_eq!(
            package.package.as_ref().map(PythonModuleName::as_str),
            Some("pkg")
        );
        assert!(package.is_package());

        for (name, expected_package) in [
            ("top_level", None),
            ("pkg.settings", Some("pkg")),
            ("pkg.__init__", Some("pkg")),
            ("app.templatetags.tags", Some("app.templatetags")),
        ] {
            let (name, path, file, search_path) = module_parts(name);
            let module = PythonSourceModule::file_module(name, path, file, search_path);
            assert_eq!(
                module.package.as_ref().map(PythonModuleName::as_str),
                expected_package
            );
            assert!(!module.is_package());
        }
    }
}

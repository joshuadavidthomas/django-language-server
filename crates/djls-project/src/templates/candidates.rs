use std::cmp::Ordering;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::FileRootKind;
use djls_source::FileSystem;
use djls_source::WalkEntryKind;
use djls_source::WalkOptions;
use djls_source::path_to_file;
use rustc_hash::FxHashMap;

use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonModule;
use crate::python::PythonModuleName;
use crate::python::SearchPath;
use crate::python::resolve_package_dirs;
use crate::templates::LibraryName;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TemplateTagDiscoveryMode {
    ActivePackage,
    AvailableCandidate,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct TemplateTagCandidate {
    pub(crate) app: PythonModuleName,
    pub(crate) name: LibraryName,
    pub(crate) module: PythonModule,
}

impl TemplateTagCandidate {
    fn from_source(
        db: &dyn ProjectDb,
        project: Project,
        source: TemplateTagCandidateSource,
    ) -> Option<Self> {
        let module_name = templatetag_module(&source.app, &source.name)
            .expect("template tag candidate source should have a valid module name");
        let file = path_to_file(db, &source.path).ok()?;
        let search_path = search_path_for_source(db, project, &source.path)?;
        Some(Self {
            app: source.app,
            name: source.name,
            module: PythonModule::new(module_name, source.path, file, search_path),
        })
    }

    fn path(&self) -> &Utf8Path {
        self.module.path()
    }

    fn into_path(self) -> Utf8PathBuf {
        self.module.path().to_path_buf()
    }

    pub(crate) fn into_python_module(self) -> PythonModule {
        self.module
    }

    fn cmp_by_app_name_path(&self, other: &Self) -> Ordering {
        self.app
            .cmp(&other.app)
            .then_with(|| self.name.cmp(&other.name))
            .then_with(|| self.path().cmp(other.path()))
    }
}

#[derive(Clone, PartialEq, Eq)]
struct TemplateTagCandidateSource {
    app: PythonModuleName,
    name: LibraryName,
    path: Utf8PathBuf,
}

impl TemplateTagCandidateSource {
    fn new(app: PythonModuleName, name: LibraryName, path: Utf8PathBuf) -> Option<Self> {
        templatetag_module(&app, &name)?;
        Some(Self { app, name, path })
    }
}

fn search_path_for_source(
    db: &dyn ProjectDb,
    project: Project,
    source_path: &Utf8Path,
) -> Option<SearchPath> {
    project
        .search_paths(db)
        .iter()
        .filter(|search_path| source_path.starts_with(search_path.path()))
        .max_by_key(|search_path| search_path.path().as_str().len())
        .cloned()
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct TemplateTagCandidates {
    candidates: Vec<TemplateTagCandidate>,
    complete: bool,
}

impl TemplateTagCandidates {
    fn new(candidates: Vec<TemplateTagCandidate>, complete: bool) -> Self {
        Self {
            candidates,
            complete,
        }
    }

    #[must_use]
    pub(crate) fn candidates(&self) -> &[TemplateTagCandidate] {
        &self.candidates
    }

    #[must_use]
    pub(crate) fn is_complete(&self) -> bool {
        self.complete
    }

    fn into_candidates(self) -> Vec<TemplateTagCandidate> {
        self.candidates
    }
}

struct TemplateTagCandidateSourceScan {
    sources: Vec<TemplateTagCandidateSource>,
    complete: bool,
}

impl TemplateTagCandidateSourceScan {
    fn new(sources: Vec<TemplateTagCandidateSource>, complete: bool) -> Self {
        Self { sources, complete }
    }
}

pub(crate) struct TemplateTagPackageScan {
    candidates: Vec<TemplateTagCandidate>,
    complete: bool,
}

impl TemplateTagPackageScan {
    fn complete() -> Self {
        Self {
            candidates: Vec::new(),
            complete: true,
        }
    }

    fn mark_incomplete(&mut self) {
        self.complete = false;
    }

    pub(crate) fn into_parts(self) -> (bool, Vec<TemplateTagCandidate>) {
        (self.complete, self.candidates)
    }
}

#[salsa::tracked(returns(ref))]
pub(crate) fn templatetag_candidates(
    db: &dyn ProjectDb,
    project: Project,
) -> TemplateTagCandidates {
    project.touch_search_path_roots(db);
    walk_candidates(db, project, TemplateTagDiscoveryMode::AvailableCandidate)
}

pub(crate) fn discover_templatetag_candidate_paths(
    db: &dyn ProjectDb,
    project: Project,
) -> Vec<Utf8PathBuf> {
    walk_candidates(db, project, TemplateTagDiscoveryMode::AvailableCandidate)
        .into_candidates()
        .into_iter()
        .map(TemplateTagCandidate::into_path)
        .collect()
}

pub(crate) fn templatetag_package_candidates(
    db: &dyn ProjectDb,
    project: Project,
    package_module: &PythonModuleName,
) -> TemplateTagPackageScan {
    let mut scan = TemplateTagPackageScan::complete();

    let package_dirs = resolve_package_dirs(db, project, package_module.clone());
    if package_dirs.dirs.is_empty() {
        scan.mark_incomplete();
        return scan;
    }

    for package_dir in package_dirs.dirs {
        let templatetags_dir = package_dir.join("templatetags");
        if !db.path_is_file(&templatetags_dir.join("__init__.py")) {
            continue;
        }

        let entries = match db.walk_entries(&templatetags_dir, &WalkOptions::shallow()) {
            Ok(entries) => entries,
            Err(err) => {
                tracing::warn!("Failed to walk template tag package {templatetags_dir}: {err}");
                scan.mark_incomplete();
                continue;
            }
        };

        for entry in entries {
            if entry.kind != WalkEntryKind::File {
                continue;
            }

            match recognize_candidate_source(
                db.file_system(),
                &package_dir,
                entry.path,
                &[],
                TemplateTagDiscoveryMode::ActivePackage,
                Some(package_module),
            ) {
                CandidateSourceRecognition::Candidate(source) => {
                    if let Some(candidate) = TemplateTagCandidate::from_source(db, project, source)
                    {
                        scan.candidates.push(candidate);
                    }
                }
                CandidateSourceRecognition::InvalidIdentifier => scan.mark_incomplete(),
                CandidateSourceRecognition::NotCandidate => {}
            }
        }
    }

    scan.candidates
        .sort_by(TemplateTagCandidate::cmp_by_app_name_path);
    scan
}

fn walk_candidates(
    db: &dyn ProjectDb,
    project: Project,
    mode: TemplateTagDiscoveryMode,
) -> TemplateTagCandidates {
    let search_paths = project.search_paths(db);
    let mut candidates_by_path: Vec<(usize, TemplateTagCandidate)> = Vec::new();
    let mut path_indexes: FxHashMap<Utf8PathBuf, usize> = FxHashMap::default();
    let mut complete = true;

    for search_path in search_paths.iter() {
        let excluded_paths = if search_path.is_first_party() {
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

        let scan = discover_templatetag_candidates(
            db,
            project,
            search_path.path(),
            search_path.root_kind(),
            &excluded_paths,
            mode,
        );
        complete &= scan.is_complete();

        for candidate in scan.into_candidates() {
            let path = candidate.path().to_path_buf();
            if let Some(index) = path_indexes.get(&path).copied() {
                let (existing_search_path_len, existing) = &mut candidates_by_path[index];
                if search_path_len > *existing_search_path_len {
                    *existing_search_path_len = search_path_len;
                    *existing = candidate;
                }
            } else {
                path_indexes.insert(path, candidates_by_path.len());
                candidates_by_path.push((search_path_len, candidate));
            }
        }
    }

    let candidates = candidates_by_path
        .into_iter()
        .map(|(_search_path_len, candidate)| candidate)
        .collect();
    TemplateTagCandidates::new(candidates, complete)
}

fn discover_templatetag_candidates(
    db: &dyn ProjectDb,
    project: Project,
    base_dir: &Utf8Path,
    root_kind: FileRootKind,
    excluded_roots: &[Utf8PathBuf],
    mode: TemplateTagDiscoveryMode,
) -> TemplateTagCandidates {
    let source_scan = discover_templatetag_candidate_sources(
        db.file_system(),
        base_dir,
        root_kind,
        excluded_roots,
        mode,
    );
    let mut results: Vec<_> = source_scan
        .sources
        .into_iter()
        .filter_map(|source| TemplateTagCandidate::from_source(db, project, source))
        .collect();

    results.sort_by(TemplateTagCandidate::cmp_by_app_name_path);
    TemplateTagCandidates::new(results, source_scan.complete)
}

fn discover_templatetag_candidate_sources(
    fs: &dyn FileSystem,
    base_dir: &Utf8Path,
    root_kind: FileRootKind,
    excluded_roots: &[Utf8PathBuf],
    mode: TemplateTagDiscoveryMode,
) -> TemplateTagCandidateSourceScan {
    let options = match root_kind {
        FileRootKind::Project => WalkOptions::project(),
        FileRootKind::SearchPath => WalkOptions::library_search_path(),
    };

    let mut results = Vec::new();
    let mut complete = true;

    let entries = match fs.walk_entries(base_dir, &options) {
        Ok(entries) => entries,
        Err(err) => {
            tracing::warn!("Failed to walk Python source root {}: {}", base_dir, err);
            return TemplateTagCandidateSourceScan::new(results, false);
        }
    };

    for entry in entries {
        if entry.kind != WalkEntryKind::File {
            continue;
        }
        let path = entry.path;
        match recognize_candidate_source(fs, base_dir, path, excluded_roots, mode, None) {
            CandidateSourceRecognition::Candidate(candidate) => results.push(candidate),
            CandidateSourceRecognition::InvalidIdentifier => complete = false,
            CandidateSourceRecognition::NotCandidate => {}
        }
    }

    TemplateTagCandidateSourceScan::new(results, complete)
}

enum CandidateSourceRecognition {
    Candidate(TemplateTagCandidateSource),
    InvalidIdentifier,
    NotCandidate,
}

fn recognize_candidate_source(
    fs: &dyn FileSystem,
    base_dir: &Utf8Path,
    path: Utf8PathBuf,
    excluded_roots: &[Utf8PathBuf],
    mode: TemplateTagDiscoveryMode,
    active_package: Option<&PythonModuleName>,
) -> CandidateSourceRecognition {
    if path.extension() != Some("py") {
        return CandidateSourceRecognition::NotCandidate;
    }
    let Some(stem) = path.file_stem() else {
        return CandidateSourceRecognition::NotCandidate;
    };
    if stem.starts_with('_') {
        return CandidateSourceRecognition::NotCandidate;
    }

    if excluded_roots
        .iter()
        .any(|excluded| path.starts_with(excluded))
    {
        return CandidateSourceRecognition::NotCandidate;
    }

    let Some(templatetags_dir) = path.parent() else {
        return CandidateSourceRecognition::NotCandidate;
    };
    if templatetags_dir.file_name() != Some("templatetags") {
        return CandidateSourceRecognition::NotCandidate;
    }
    if !fs.exists(&templatetags_dir.join("__init__.py")) {
        return CandidateSourceRecognition::NotCandidate;
    }

    let app = match mode {
        TemplateTagDiscoveryMode::ActivePackage => {
            let Some(package_module) = active_package else {
                return CandidateSourceRecognition::NotCandidate;
            };
            package_module.clone()
        }
        TemplateTagDiscoveryMode::AvailableCandidate => {
            let Some(app_dir) = templatetags_dir.parent() else {
                return CandidateSourceRecognition::NotCandidate;
            };
            if app_dir == base_dir || !fs.exists(&app_dir.join("__init__.py")) {
                return CandidateSourceRecognition::NotCandidate;
            }
            let Ok(app_rel) = app_dir.strip_prefix(base_dir) else {
                return CandidateSourceRecognition::NotCandidate;
            };
            let Ok(app) = PythonModuleName::from_relative_package_path(app_rel) else {
                return CandidateSourceRecognition::NotCandidate;
            };
            app
        }
    };

    let Ok(name) = LibraryName::parse(stem) else {
        return CandidateSourceRecognition::InvalidIdentifier;
    };
    let Some(candidate) = TemplateTagCandidateSource::new(app, name, path) else {
        return CandidateSourceRecognition::InvalidIdentifier;
    };
    CandidateSourceRecognition::Candidate(candidate)
}

fn templatetag_module(app: &PythonModuleName, name: &LibraryName) -> Option<PythonModuleName> {
    PythonModuleName::parse(&format!("{}.templatetags.{}", app.as_str(), name.as_str())).ok()
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;
    use djls_source::InMemoryFileSystem;

    use super::*;

    #[test]
    fn discover_templatetag_candidates_requires_django_package_shape() {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file("/root/pkg_a/__init__.py".into(), String::new());
        fs.add_file("/root/pkg_a/templatetags/__init__.py".into(), String::new());
        fs.add_file("/root/pkg_a/templatetags/foo.py".into(), String::new());
        fs.add_file("/root/pkg_a/templatetags/_private.py".into(), String::new());
        fs.add_file("/root/pkg_b/__init__.py".into(), String::new());
        fs.add_file("/root/pkg_b/templatetags/bar.py".into(), String::new());
        fs.add_file("/root/loose/templatetags/__init__.py".into(), String::new());
        fs.add_file("/root/loose/templatetags/baz.py".into(), String::new());

        let discovered = discover_templatetag_candidate_sources(
            &fs,
            Utf8Path::new("/root"),
            FileRootKind::Project,
            &[],
            TemplateTagDiscoveryMode::AvailableCandidate,
        );

        assert!(discovered.complete);
        assert_eq!(discovered.sources.len(), 1);
        let candidate = &discovered.sources[0];
        assert_eq!(candidate.app.as_str(), "pkg_a");
        assert_eq!(candidate.name.as_str(), "foo");
        assert_eq!(candidate.path.as_str(), "/root/pkg_a/templatetags/foo.py");
    }

    #[test]
    fn recognizer_modes_keep_active_and_available_package_shape_distinct() {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(
            "/root/namespace_app/templatetags/__init__.py".into(),
            String::new(),
        );
        fs.add_file(
            "/root/namespace_app/templatetags/tools.py".into(),
            String::new(),
        );

        let path = Utf8PathBuf::from("/root/namespace_app/templatetags/tools.py");
        let package = PythonModuleName::parse("namespace_app").unwrap();
        let active = recognize_candidate_source(
            &fs,
            Utf8Path::new("/root/namespace_app"),
            path.clone(),
            &[],
            TemplateTagDiscoveryMode::ActivePackage,
            Some(&package),
        );
        let available = recognize_candidate_source(
            &fs,
            Utf8Path::new("/root"),
            path,
            &[],
            TemplateTagDiscoveryMode::AvailableCandidate,
            None,
        );

        let CandidateSourceRecognition::Candidate(candidate) = active else {
            panic!("active package scan should accept namespace package templatetags");
        };
        assert_eq!(candidate.app.as_str(), "namespace_app");
        assert_eq!(candidate.name.as_str(), "tools");
        assert!(matches!(
            available,
            CandidateSourceRecognition::NotCandidate
        ));
    }
}

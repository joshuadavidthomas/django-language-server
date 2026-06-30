use std::cmp::Ordering;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::FileRootKind;
use djls_source::FileSystem;
use djls_source::WalkEntryKind;
use djls_source::WalkOptions;
use rustc_hash::FxHashMap;

use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonModule;
use crate::python::PythonModuleName;
use crate::python::PythonPackage;
use crate::settings::StaticKnowledge;
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
    fn from_source(db: &dyn ProjectDb, source: TemplateTagCandidateSource) -> Self {
        let module_name = templatetag_module(&source.app, &source.name)
            .expect("template tag candidate source should have a valid module name");
        let file = db.get_or_create_file(&source.path);
        Self {
            app: source.app,
            name: source.name,
            module: PythonModule::new(module_name, source.path, file),
        }
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

pub(crate) struct TemplateTagPackageScan {
    candidates: Vec<TemplateTagCandidate>,
    knowledge: StaticKnowledge,
}

impl TemplateTagPackageScan {
    fn known() -> Self {
        Self {
            candidates: Vec::new(),
            knowledge: StaticKnowledge::Known,
        }
    }

    fn demote_to_partial(&mut self) {
        self.knowledge = self.knowledge.demoted_to_partial();
    }

    pub(crate) fn into_parts(self) -> (StaticKnowledge, Vec<TemplateTagCandidate>) {
        (self.knowledge, self.candidates)
    }
}

#[salsa::tracked(returns(ref))]
pub(crate) fn templatetag_candidates(
    db: &dyn ProjectDb,
    project: Project,
) -> Vec<TemplateTagCandidate> {
    project.touch_search_path_roots(db);
    walk_candidates(db, project, TemplateTagDiscoveryMode::AvailableCandidate)
}

pub(crate) fn refresh_templatetag_candidate_paths(
    db: &dyn ProjectDb,
    project: Project,
) -> Vec<Utf8PathBuf> {
    walk_candidates(db, project, TemplateTagDiscoveryMode::AvailableCandidate)
        .into_iter()
        .map(TemplateTagCandidate::into_path)
        .collect()
}

pub(crate) fn templatetag_package_candidates(
    db: &dyn ProjectDb,
    project: Project,
    package_module: PythonModuleName,
) -> TemplateTagPackageScan {
    let mut scan = TemplateTagPackageScan::known();

    let Some(package) = PythonPackage::resolve(db, project, package_module) else {
        scan.demote_to_partial();
        return scan;
    };

    let templatetags_dir = package.dir().join("templatetags");
    if !db.path_is_file(&templatetags_dir.join("__init__.py")) {
        return scan;
    }

    let entries = match db.walk_entries(&templatetags_dir, &WalkOptions::shallow()) {
        Ok(entries) => entries,
        Err(err) => {
            tracing::warn!("Failed to walk template tag package {templatetags_dir}: {err}");
            scan.demote_to_partial();
            return scan;
        }
    };

    for entry in entries {
        if entry.kind != WalkEntryKind::File {
            continue;
        }

        match recognize_candidate_source(
            db.file_system(),
            package.dir(),
            entry.path,
            &[],
            TemplateTagDiscoveryMode::ActivePackage,
            Some(package.name()),
        ) {
            CandidateSourceRecognition::Candidate(source) => {
                scan.candidates
                    .push(TemplateTagCandidate::from_source(db, source));
            }
            CandidateSourceRecognition::InvalidIdentifier => scan.demote_to_partial(),
            CandidateSourceRecognition::NotCandidate => {}
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
) -> Vec<TemplateTagCandidate> {
    let search_paths = project.search_paths(db);
    let mut candidates_by_path: Vec<(usize, TemplateTagCandidate)> = Vec::new();
    let mut path_indexes: FxHashMap<Utf8PathBuf, usize> = FxHashMap::default();

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

        for candidate in discover_templatetag_candidates(
            db,
            search_path.path(),
            search_path.root_kind(),
            &excluded_paths,
            mode,
        ) {
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

    candidates_by_path
        .into_iter()
        .map(|(_search_path_len, candidate)| candidate)
        .collect()
}

fn discover_templatetag_candidates(
    db: &dyn ProjectDb,
    base_dir: &Utf8Path,
    root_kind: FileRootKind,
    excluded_roots: &[Utf8PathBuf],
    mode: TemplateTagDiscoveryMode,
) -> Vec<TemplateTagCandidate> {
    let mut results: Vec<_> = discover_templatetag_candidate_sources(
        db.file_system(),
        base_dir,
        root_kind,
        excluded_roots,
        mode,
    )
    .into_iter()
    .map(|source| TemplateTagCandidate::from_source(db, source))
    .collect();

    results.sort_by(TemplateTagCandidate::cmp_by_app_name_path);
    results
}

fn discover_templatetag_candidate_sources(
    fs: &dyn FileSystem,
    base_dir: &Utf8Path,
    root_kind: FileRootKind,
    excluded_roots: &[Utf8PathBuf],
    mode: TemplateTagDiscoveryMode,
) -> Vec<TemplateTagCandidateSource> {
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
        match recognize_candidate_source(fs, base_dir, path, excluded_roots, mode, None) {
            CandidateSourceRecognition::Candidate(candidate) => results.push(candidate),
            CandidateSourceRecognition::InvalidIdentifier
            | CandidateSourceRecognition::NotCandidate => {}
        }
    }

    results
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

        assert_eq!(discovered.len(), 1);
        let candidate = &discovered[0];
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

use std::cmp::Ordering;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::FileRootKind;
use djls_source::FileSystem;
use djls_source::RootWalk;
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

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct TemplateTagCandidate {
    pub(crate) app: PythonModuleName,
    pub(crate) name: LibraryName,
    pub(crate) module: PythonModule,
}

impl TemplateTagCandidate {
    fn from_parts(
        db: &dyn ProjectDb,
        project: Project,
        app: PythonModuleName,
        name: LibraryName,
        path: Utf8PathBuf,
    ) -> Result<Self, TemplateTagCandidateIssue> {
        let module_name = templatetag_module(&app, &name)
            .expect("recognized template tag candidate should have a valid module name");
        let package = module_name.parent();
        let file =
            path_to_file(db, &path).map_err(|_| TemplateTagCandidateIssue::FileConversion)?;
        let search_path = search_path_for_source(db, project, &path)
            .ok_or(TemplateTagCandidateIssue::RootAssociation)?;
        Ok(Self {
            app,
            name,
            module: PythonModule::new(module_name, package, path, file, search_path),
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TemplateTagCandidateIssue {
    PackageResolution,
    Walk,
    InvalidIdentifier,
    FileConversion,
    RootAssociation,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum TemplateTagCandidateScan {
    Exhaustive(Vec<TemplateTagCandidate>),
    WithOmissions {
        candidates: Vec<TemplateTagCandidate>,
        omissions: Vec<TemplateTagCandidateIssue>,
    },
}

impl TemplateTagCandidateScan {
    fn new() -> Self {
        Self::Exhaustive(Vec::new())
    }

    #[must_use]
    pub(crate) fn candidates(&self) -> &[TemplateTagCandidate] {
        match self {
            Self::Exhaustive(candidates) | Self::WithOmissions { candidates, .. } => candidates,
        }
    }

    #[must_use]
    pub(crate) const fn has_omissions(&self) -> bool {
        matches!(self, Self::WithOmissions { .. })
    }

    fn candidates_mut(&mut self) -> &mut Vec<TemplateTagCandidate> {
        match self {
            Self::Exhaustive(candidates) | Self::WithOmissions { candidates, .. } => candidates,
        }
    }

    fn candidate(&mut self, candidate: TemplateTagCandidate) {
        self.candidates_mut().push(candidate);
    }

    fn issue(&mut self, issue: TemplateTagCandidateIssue) {
        match self {
            Self::Exhaustive(candidates) => {
                let candidates = std::mem::take(candidates);
                *self = Self::WithOmissions {
                    candidates,
                    omissions: vec![issue],
                };
            }
            Self::WithOmissions { omissions, .. } => omissions.push(issue),
        }
    }

    fn extend(&mut self, other: Self) {
        let (candidates, issues) = other.into_parts();
        self.candidates_mut().extend(candidates);
        for issue in issues {
            self.issue(issue);
        }
    }

    fn sort(&mut self) {
        self.candidates_mut()
            .sort_by(TemplateTagCandidate::cmp_by_app_name_path);
    }

    fn into_candidates(self) -> Vec<TemplateTagCandidate> {
        match self {
            Self::Exhaustive(candidates) | Self::WithOmissions { candidates, .. } => candidates,
        }
    }

    pub(crate) fn into_parts(self) -> (Vec<TemplateTagCandidate>, Vec<TemplateTagCandidateIssue>) {
        match self {
            Self::Exhaustive(candidates) => (candidates, Vec::new()),
            Self::WithOmissions {
                candidates,
                omissions,
            } => (candidates, omissions),
        }
    }
}

#[salsa::tracked(returns(ref))]
pub(crate) fn templatetag_candidates(
    db: &dyn ProjectDb,
    project: Project,
) -> TemplateTagCandidateScan {
    project.touch_search_path_roots(db);
    search_path_templatetag_candidates(db, project)
}

pub(crate) fn discover_templatetag_candidate_paths(
    db: &dyn ProjectDb,
    project: Project,
) -> Vec<Utf8PathBuf> {
    search_path_templatetag_candidates(db, project)
        .into_candidates()
        .into_iter()
        .map(TemplateTagCandidate::into_path)
        .collect()
}

pub(crate) fn templatetag_candidates_in_package(
    db: &dyn ProjectDb,
    project: Project,
    package_module: &PythonModuleName,
) -> TemplateTagCandidateScan {
    let mut scan = TemplateTagCandidateScan::new();

    let package_dirs = resolve_package_dirs(db, project, package_module.clone());
    if package_dirs.dirs.is_empty() {
        scan.issue(TemplateTagCandidateIssue::PackageResolution);
        return scan;
    }

    for package_dir in package_dirs.dirs {
        let templatetags_dir = package_dir.join("templatetags");
        if !db.path_is_file(&templatetags_dir.join("__init__.py")) {
            continue;
        }

        scan.extend(scan_templatetag_root(
            db,
            project,
            &templatetags_dir,
            &WalkOptions::shallow(),
            &[],
            Some(package_module),
        ));
    }

    scan.sort();
    scan
}

fn search_path_templatetag_candidates(
    db: &dyn ProjectDb,
    project: Project,
) -> TemplateTagCandidateScan {
    let search_paths = project.search_paths(db);
    let mut candidates_by_path: Vec<(usize, TemplateTagCandidate)> = Vec::new();
    let mut path_indexes: FxHashMap<Utf8PathBuf, usize> = FxHashMap::default();
    let mut issues = Vec::new();

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

        let options = match search_path.root_kind() {
            FileRootKind::Project => WalkOptions::project(),
            FileRootKind::SearchPath => WalkOptions::library_search_path(),
        };
        let scan = scan_templatetag_root(
            db,
            project,
            search_path.path(),
            &options,
            &excluded_paths,
            None,
        );
        let (scan_candidates, scan_issues) = scan.into_parts();
        issues.extend(scan_issues);

        for candidate in scan_candidates {
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
    let mut scan = TemplateTagCandidateScan::Exhaustive(candidates);
    for issue in issues {
        scan.issue(issue);
    }
    scan
}

fn scan_templatetag_root(
    db: &dyn ProjectDb,
    project: Project,
    base_dir: &Utf8Path,
    options: &WalkOptions,
    excluded_roots: &[Utf8PathBuf],
    active_package: Option<&PythonModuleName>,
) -> TemplateTagCandidateScan {
    let fs = db.file_system();
    let mut scan = TemplateTagCandidateScan::new();

    let entries = match fs.walk_root(base_dir, options) {
        RootWalk::Directory {
            entries,
            issues: walk_issues,
        } => {
            if !walk_issues.is_empty() {
                tracing::warn!(
                    "Partially walked Python source root {}: {:?}",
                    base_dir,
                    walk_issues
                );
                scan.issue(TemplateTagCandidateIssue::Walk);
            }
            entries
        }
        RootWalk::Missing | RootWalk::File(_) => Vec::new(),
        RootWalk::Inaccessible(kind) => {
            tracing::warn!("Failed to walk Python source root {}: {:?}", base_dir, kind);
            scan.issue(TemplateTagCandidateIssue::Walk);
            return scan;
        }
    };

    for entry in entries {
        if entry.kind != WalkEntryKind::File {
            continue;
        }
        let path = entry.path;
        match recognize_candidate_source(fs, base_dir, path, excluded_roots, active_package) {
            CandidateSourceRecognition::Candidate { app, name, path } => {
                match TemplateTagCandidate::from_parts(db, project, app, name, path) {
                    Ok(candidate) => scan.candidate(candidate),
                    Err(issue) => scan.issue(issue),
                }
            }
            CandidateSourceRecognition::InvalidIdentifier => {
                scan.issue(TemplateTagCandidateIssue::InvalidIdentifier);
            }
            CandidateSourceRecognition::NotCandidate => {}
        }
    }

    scan.sort();
    scan
}

enum CandidateSourceRecognition {
    Candidate {
        app: PythonModuleName,
        name: LibraryName,
        path: Utf8PathBuf,
    },
    InvalidIdentifier,
    NotCandidate,
}

fn recognize_candidate_source(
    fs: &dyn FileSystem,
    base_dir: &Utf8Path,
    path: Utf8PathBuf,
    excluded_roots: &[Utf8PathBuf],
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

    let app = if let Some(package_module) = active_package {
        package_module.clone()
    } else {
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
    };

    let Ok(name) = LibraryName::parse(stem) else {
        return CandidateSourceRecognition::InvalidIdentifier;
    };
    if templatetag_module(&app, &name).is_none() {
        return CandidateSourceRecognition::InvalidIdentifier;
    }
    CandidateSourceRecognition::Candidate { app, name, path }
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

        let paths = [
            "/root/pkg_a/templatetags/foo.py",
            "/root/pkg_a/templatetags/_private.py",
            "/root/pkg_b/templatetags/bar.py",
            "/root/loose/templatetags/baz.py",
        ];
        let discovered = paths
            .into_iter()
            .filter_map(|path| {
                match recognize_candidate_source(
                    &fs,
                    Utf8Path::new("/root"),
                    path.into(),
                    &[],
                    None,
                ) {
                    CandidateSourceRecognition::Candidate { app, name, path } => {
                        Some((app, name, path))
                    }
                    CandidateSourceRecognition::InvalidIdentifier
                    | CandidateSourceRecognition::NotCandidate => None,
                }
            })
            .collect::<Vec<_>>();

        assert_eq!(discovered.len(), 1);
        let (app, name, path) = &discovered[0];
        assert_eq!(app.as_str(), "pkg_a");
        assert_eq!(name.as_str(), "foo");
        assert_eq!(path.as_str(), "/root/pkg_a/templatetags/foo.py");
    }

    #[test]
    fn known_package_identity_accepts_namespace_package_shape() {
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
            Some(&package),
        );
        let available = recognize_candidate_source(&fs, Utf8Path::new("/root"), path, &[], None);

        let CandidateSourceRecognition::Candidate { app, name, .. } = active else {
            panic!("active package scan should accept namespace package templatetags");
        };
        assert_eq!(app.as_str(), "namespace_app");
        assert_eq!(name.as_str(), "tools");
        assert!(matches!(
            available,
            CandidateSourceRecognition::NotCandidate
        ));
    }
}

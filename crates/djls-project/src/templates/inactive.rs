use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::LazyLock;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::FileRootKind;
use djls_source::FileSystem;
use djls_source::WalkEntryKind;
use djls_source::WalkOptions;
use rustc_hash::FxHashMap;

use super::names::LibraryName;
use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonModulePath;
use crate::templates::TemplateLibraryAnalysis;
use crate::templates::TemplateSymbolKind;
use crate::templates::template_libraries;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InactiveLibrary {
    pub name: LibraryName,
    pub app: PythonModulePath,
    pub module: PythonModulePath,
    pub tags: Vec<String>,
    pub filters: Vec<String>,
}

impl InactiveLibrary {
    fn from_candidate(
        db: &dyn ProjectDb,
        candidate: TemplateTagCandidate,
        active_modules: &BTreeSet<PythonModulePath>,
    ) -> Option<Self> {
        if active_modules.contains(&candidate.module) {
            return None;
        }

        let file = db.get_or_create_file(&candidate.path);
        let analysis = TemplateLibraryAnalysis::from_file(db, file);
        if !analysis.defines_library && analysis.symbols.is_empty() {
            return None;
        }

        let mut tags = Vec::new();
        let mut filters = Vec::new();
        for symbol in analysis.symbols {
            match symbol.kind {
                TemplateSymbolKind::Tag => tags.push(symbol.name.as_str().to_string()),
                TemplateSymbolKind::Filter => filters.push(symbol.name.as_str().to_string()),
            }
        }
        tags.sort();
        tags.dedup();
        filters.sort();
        filters.dedup();

        Some(Self {
            name: candidate.name,
            app: candidate.app,
            module: candidate.module,
            tags,
            filters,
        })
    }

    fn cmp_by_app_name_module(&self, other: &Self) -> Ordering {
        self.app
            .cmp(&other.app)
            .then_with(|| self.name.cmp(&other.name))
            .then_with(|| self.module.cmp(&other.module))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InactiveLibraries {
    pub by_name: BTreeMap<LibraryName, Vec<InactiveLibrary>>,
}

impl InactiveLibraries {
    #[must_use]
    pub fn empty_ref() -> &'static Self {
        static EMPTY: LazyLock<InactiveLibraries> = LazyLock::new(InactiveLibraries::default);
        &EMPTY
    }

    fn push(&mut self, library: InactiveLibrary) {
        self.by_name
            .entry(library.name.clone())
            .or_default()
            .push(library);
    }

    fn sort_and_dedup(&mut self) {
        for libraries in self.by_name.values_mut() {
            libraries.sort_by(InactiveLibrary::cmp_by_app_name_module);
            libraries.dedup_by(|left, right| left.app == right.app && left.module == right.module);
        }
    }

    #[must_use]
    pub fn library_candidates(&self, name: &LibraryName) -> &[InactiveLibrary] {
        self.by_name.get(name).map_or(&[], Vec::as_slice)
    }

    #[must_use]
    pub fn tag_candidates(&self, tag: &str) -> Vec<&InactiveLibrary> {
        let mut candidates: Vec<_> = self
            .by_name
            .values()
            .flat_map(|libraries| libraries.iter())
            .filter(|library| library.tags.iter().any(|candidate| candidate == tag))
            .collect();
        candidates.sort_by(|left, right| left.cmp_by_app_name_module(right));
        candidates
    }

    #[must_use]
    pub fn filter_candidates(&self, filter: &str) -> Vec<&InactiveLibrary> {
        let mut candidates: Vec<_> = self
            .by_name
            .values()
            .flat_map(|libraries| libraries.iter())
            .filter(|library| library.filters.iter().any(|candidate| candidate == filter))
            .collect();
        candidates.sort_by(|left, right| left.cmp_by_app_name_module(right));
        candidates
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TemplateTagCandidate {
    app: PythonModulePath,
    name: LibraryName,
    module: PythonModulePath,
    path: Utf8PathBuf,
}

impl TemplateTagCandidate {
    fn new(app: PythonModulePath, name: LibraryName, path: Utf8PathBuf) -> Option<Self> {
        let module =
            PythonModulePath::parse(&format!("{}.templatetags.{}", app.as_str(), name.as_str()))
                .ok()?;

        Some(Self {
            app,
            name,
            module,
            path,
        })
    }

    fn into_path(self) -> Utf8PathBuf {
        self.path
    }

    fn cmp_by_app_name_path(&self, other: &Self) -> Ordering {
        self.app
            .cmp(&other.app)
            .then_with(|| self.name.cmp(&other.name))
            .then_with(|| self.path.cmp(&other.path))
    }
}

#[salsa::tracked(returns(ref))]
pub fn inactive_template_libraries(db: &dyn ProjectDb, project: Project) -> InactiveLibraries {
    project.touch_search_path_roots(db);

    let template_libraries = template_libraries(db, project);
    let active_modules: BTreeSet<_> = template_libraries
        .loadable
        .values()
        .map(|library| library.module().clone())
        .chain(template_libraries.builtin_modules().cloned())
        .collect();

    let mut inactive = InactiveLibraries::default();
    for candidate in discover_project_templatetag_candidates(db, project) {
        if let Some(library) = InactiveLibrary::from_candidate(db, candidate, &active_modules) {
            inactive.push(library);
        }
    }

    inactive.sort_and_dedup();
    inactive
}

pub(crate) fn templatetag_candidate_paths(
    db: &dyn ProjectDb,
    project: Project,
) -> Vec<Utf8PathBuf> {
    discover_project_templatetag_candidates(db, project)
        .into_iter()
        .map(TemplateTagCandidate::into_path)
        .collect()
}

fn discover_project_templatetag_candidates(
    db: &dyn ProjectDb,
    project: Project,
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
            db.file_system(),
            search_path.path(),
            search_path.root_kind(),
            &excluded_paths,
        ) {
            let path = candidate.path.clone();
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
    fs: &dyn FileSystem,
    base_dir: &Utf8Path,
    root_kind: FileRootKind,
    excluded_roots: &[Utf8PathBuf],
) -> Vec<TemplateTagCandidate> {
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

        if path.extension() != Some("py") {
            continue;
        }
        let Some(stem) = path.file_stem() else {
            continue;
        };
        if stem.starts_with('_') {
            continue;
        }

        if excluded_roots
            .iter()
            .any(|excluded| path.starts_with(excluded))
        {
            continue;
        }

        let Some(templatetags_dir) = path.parent() else {
            continue;
        };
        if templatetags_dir.file_name() != Some("templatetags") {
            continue;
        }
        if !fs.exists(&templatetags_dir.join("__init__.py")) {
            continue;
        }

        let Some(app_dir) = templatetags_dir.parent() else {
            continue;
        };
        if app_dir == base_dir || !fs.exists(&app_dir.join("__init__.py")) {
            continue;
        }

        let Some(app_rel) = app_dir.strip_prefix(base_dir).ok() else {
            continue;
        };
        let Ok(app) = PythonModulePath::from_relative_package(app_rel) else {
            continue;
        };
        let Ok(name) = LibraryName::parse(stem) else {
            continue;
        };
        let Some(candidate) = TemplateTagCandidate::new(app, name, path) else {
            continue;
        };

        results.push(candidate);
    }

    results.sort_by(TemplateTagCandidate::cmp_by_app_name_path);
    results
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

        let discovered = discover_templatetag_candidates(
            &fs,
            Utf8Path::new("/root"),
            FileRootKind::Project,
            &[],
        );

        assert_eq!(discovered.len(), 1);
        let candidate = &discovered[0];
        assert_eq!(candidate.app.as_str(), "pkg_a");
        assert_eq!(candidate.name.as_str(), "foo");
        assert_eq!(candidate.module.as_str(), "pkg_a.templatetags.foo");
        assert_eq!(candidate.path.as_str(), "/root/pkg_a/templatetags/foo.py");
    }
}

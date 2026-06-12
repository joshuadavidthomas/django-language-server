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

use crate::db::Db as ProjectDb;
use crate::names::LibraryName;
use crate::names::PyModuleName;
use crate::project::Project;
use crate::settings::TemplateLibraryAnalysis;
use crate::settings::template_libraries;
use crate::symbols::TemplateLibraries;
use crate::symbols::TemplateSymbolKind;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InactiveLibrary {
    pub name: LibraryName,
    pub app: PyModuleName,
    pub module: PyModuleName,
    pub tags: Vec<String>,
    pub filters: Vec<String>,
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
        sort_candidates(&mut candidates);
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
        sort_candidates(&mut candidates);
        candidates
    }
}

fn sort_candidates(candidates: &mut [&InactiveLibrary]) {
    candidates.sort_by(|left, right| {
        left.app
            .cmp(&right.app)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.module.cmp(&right.module))
    });
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TemplateTagFile {
    app: PyModuleName,
    name: LibraryName,
    module: PyModuleName,
    path: Utf8PathBuf,
}

#[salsa::tracked(returns(ref))]
pub fn inactive_template_libraries(db: &dyn ProjectDb, project: Project) -> InactiveLibraries {
    let active_modules = active_template_library_modules(template_libraries(db, project));
    let mut inactive = InactiveLibraries::default();

    for candidate in discover_project_templatetag_files(db, project) {
        let Some(library) = inactive_library_from_candidate(db, candidate, &active_modules) else {
            continue;
        };
        inactive
            .by_name
            .entry(library.name.clone())
            .or_default()
            .push(library);
    }

    sort_inactive_libraries(&mut inactive);
    inactive
}

pub(crate) fn templatetag_candidate_paths(
    db: &dyn ProjectDb,
    project: Project,
) -> Vec<Utf8PathBuf> {
    discover_project_templatetag_files(db, project)
        .into_iter()
        .map(|file| file.path)
        .collect()
}

fn discover_project_templatetag_files(
    db: &dyn ProjectDb,
    project: Project,
) -> Vec<TemplateTagFile> {
    let search_paths = project.search_paths(db);
    let mut files_by_path: Vec<(usize, TemplateTagFile)> = Vec::new();
    let mut path_indexes: FxHashMap<Utf8PathBuf, usize> = FxHashMap::default();

    for search_path in search_paths.iter() {
        touch_search_path_root(db, search_path.path());
        let excluded_paths = if search_path.is_first_party() {
            nested_non_first_party_paths(project, db, search_path.path())
        } else {
            Vec::new()
        };
        let search_path_len = search_path.path().as_str().len();

        for (app, name, path) in discover_templatetag_files(
            db.file_system(),
            search_path.path(),
            search_path.root_kind(),
            &excluded_paths,
        ) {
            let Some(module) = module_from_app_and_library(&app, &name) else {
                continue;
            };
            let file = TemplateTagFile {
                app,
                name,
                module,
                path: path.clone(),
            };
            record_longest_search_path_file(
                &mut files_by_path,
                &mut path_indexes,
                search_path_len,
                path,
                file,
            );
        }
    }

    files_by_path
        .into_iter()
        .map(|(_search_path_len, file)| file)
        .collect()
}

fn touch_search_path_root(db: &dyn ProjectDb, path: &Utf8Path) {
    if let Some(root) = db.files().root(db, path) {
        let _ = root.revision(db);
    } else {
        tracing::warn!("Search path has no registered source root: {}", path);
    }
}

fn nested_non_first_party_paths(
    project: Project,
    db: &dyn ProjectDb,
    search_path: &Utf8Path,
) -> Vec<Utf8PathBuf> {
    project
        .search_paths(db)
        .iter()
        .filter(|other| !other.is_first_party() && other.path().starts_with(search_path))
        .map(|other| other.path().to_path_buf())
        .collect()
}

fn record_longest_search_path_file(
    files_by_path: &mut Vec<(usize, TemplateTagFile)>,
    path_indexes: &mut FxHashMap<Utf8PathBuf, usize>,
    search_path_len: usize,
    path: Utf8PathBuf,
    file: TemplateTagFile,
) {
    if let Some(index) = path_indexes.get(&path).copied() {
        let (existing_search_path_len, existing) = &mut files_by_path[index];
        if search_path_len > *existing_search_path_len {
            *existing_search_path_len = search_path_len;
            *existing = file;
        }
    } else {
        path_indexes.insert(path, files_by_path.len());
        files_by_path.push((search_path_len, file));
    }
}

fn active_template_library_modules(libraries: &TemplateLibraries) -> BTreeSet<PyModuleName> {
    libraries
        .loadable
        .values()
        .map(|library| library.module().clone())
        .chain(libraries.builtin_modules().cloned())
        .collect()
}

fn inactive_library_from_candidate(
    db: &dyn ProjectDb,
    candidate: TemplateTagFile,
    active_modules: &BTreeSet<PyModuleName>,
) -> Option<InactiveLibrary> {
    if active_modules.contains(&candidate.module) {
        return None;
    }

    let file = db.get_or_create_file(&candidate.path);
    let analysis = TemplateLibraryAnalysis::from_file(db, file);
    if !analysis.defines_library && analysis.symbols.is_empty() {
        return None;
    }

    let (tags, filters) = sorted_symbol_names(analysis);
    Some(InactiveLibrary {
        name: candidate.name,
        app: candidate.app,
        module: candidate.module,
        tags,
        filters,
    })
}

fn sorted_symbol_names(analysis: TemplateLibraryAnalysis) -> (Vec<String>, Vec<String>) {
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
    (tags, filters)
}

fn sort_inactive_libraries(inactive: &mut InactiveLibraries) {
    for libraries in inactive.by_name.values_mut() {
        libraries.sort_by(|left, right| {
            left.app
                .cmp(&right.app)
                .then_with(|| left.module.cmp(&right.module))
        });
        libraries.dedup_by(|left, right| left.app == right.app && left.module == right.module);
    }
}

fn discover_templatetag_files(
    fs: &dyn FileSystem,
    base_dir: &Utf8Path,
    root_kind: FileRootKind,
    excluded_roots: &[Utf8PathBuf],
) -> Vec<(PyModuleName, LibraryName, Utf8PathBuf)> {
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
        let Ok(app) = PyModuleName::from_relative_package(app_rel) else {
            continue;
        };
        let Ok(name) = LibraryName::parse(stem) else {
            continue;
        };

        results.push((app, name, path));
    }

    results.sort_by(
        |(left_app, left_name, left_path), (right_app, right_name, right_path)| {
            left_app
                .cmp(right_app)
                .then_with(|| left_name.cmp(right_name))
                .then_with(|| left_path.cmp(right_path))
        },
    );
    results
}

fn module_from_app_and_library(app: &PyModuleName, name: &LibraryName) -> Option<PyModuleName> {
    PyModuleName::parse(&format!("{}.templatetags.{}", app.as_str(), name.as_str())).ok()
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;
    use djls_source::InMemoryFileSystem;

    use super::*;

    #[test]
    fn discover_templatetag_files_requires_django_package_shape() {
        let mut fs = InMemoryFileSystem::new();
        fs.add_file("/root/pkg_a/__init__.py".into(), String::new());
        fs.add_file("/root/pkg_a/templatetags/__init__.py".into(), String::new());
        fs.add_file("/root/pkg_a/templatetags/foo.py".into(), String::new());
        fs.add_file("/root/pkg_a/templatetags/_private.py".into(), String::new());
        fs.add_file("/root/pkg_b/__init__.py".into(), String::new());
        fs.add_file("/root/pkg_b/templatetags/bar.py".into(), String::new());
        fs.add_file("/root/loose/templatetags/__init__.py".into(), String::new());
        fs.add_file("/root/loose/templatetags/baz.py".into(), String::new());

        let discovered =
            discover_templatetag_files(&fs, Utf8Path::new("/root"), FileRootKind::Project, &[]);

        assert_eq!(discovered.len(), 1);
        let (app, name, path) = &discovered[0];
        assert_eq!(app.as_str(), "pkg_a");
        assert_eq!(name.as_str(), "foo");
        assert_eq!(path.as_str(), "/root/pkg_a/templatetags/foo.py");
    }
}

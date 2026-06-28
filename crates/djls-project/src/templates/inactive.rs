use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::LazyLock;

use super::names::LibraryName;
use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonModulePath;
use crate::templates::TemplateLibraryAnalysis;
use crate::templates::TemplateSymbolKind;
use crate::templates::TemplateTagCandidate;
use crate::templates::template_libraries;
use crate::templates::templatetag_candidates;

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

        let analysis = TemplateLibraryAnalysis::from_file(db, candidate.file);
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
    by_name: BTreeMap<LibraryName, Vec<InactiveLibrary>>,
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
    for candidate in templatetag_candidates(db, project).iter().cloned() {
        if let Some(library) = InactiveLibrary::from_candidate(db, candidate, &active_modules) {
            inactive.push(library);
        }
    }

    inactive.sort_and_dedup();
    inactive
}

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::collections::BTreeSet;

use djls_source::File;

use super::candidates::templatetag_candidates;
use super::candidates::templatetag_package_candidates;
use super::guess_package_module_from_installed_app_entry;
use super::names::LibraryName;
use super::registrations::TemplateLibraryAnalysis;
use super::symbols::TemplateSymbol;
use super::symbols::TemplateSymbolKind;
use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonModule;
use crate::python::PythonModuleName;
use crate::python::PythonResolver;
use crate::settings::StaticKnowledge;
use crate::settings::django_settings;
use crate::settings::settings_module_file;

const DEFAULT_TEMPLATE_BUILTINS: &[&str] = &[
    "django.template.defaulttags",
    "django.template.defaultfilters",
    "django.template.loader_tags",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TemplateLibraryStatus {
    Builtin,
    Installed,
    Available,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateLibrary {
    load_name: Option<LibraryName>,
    app: Option<PythonModuleName>,
    module: PythonModule,
    status: TemplateLibraryStatus,
    symbols: Vec<TemplateSymbol>,
}

impl TemplateLibrary {
    #[must_use]
    pub(crate) fn builtin(module: PythonModule, symbols: Vec<TemplateSymbol>) -> Self {
        Self {
            load_name: None,
            app: None,
            module,
            status: TemplateLibraryStatus::Builtin,
            symbols: merge_symbols(symbols),
        }
    }

    #[must_use]
    pub(crate) fn installed(
        load_name: LibraryName,
        module: PythonModule,
        symbols: Vec<TemplateSymbol>,
    ) -> Self {
        Self {
            load_name: Some(load_name),
            app: None,
            module,
            status: TemplateLibraryStatus::Installed,
            symbols: merge_symbols(symbols),
        }
    }

    #[must_use]
    pub(crate) fn available(
        load_name: LibraryName,
        app: PythonModuleName,
        module: PythonModule,
        symbols: Vec<TemplateSymbol>,
    ) -> Self {
        Self {
            load_name: Some(load_name),
            app: Some(app),
            module,
            status: TemplateLibraryStatus::Available,
            symbols: merge_symbols(symbols),
        }
    }

    #[must_use]
    pub fn load_name(&self) -> Option<&LibraryName> {
        self.load_name.as_ref()
    }

    #[must_use]
    pub fn module_name(&self) -> &PythonModuleName {
        self.module.name()
    }

    #[must_use]
    pub fn module_name_str(&self) -> &str {
        self.module.name().as_str()
    }

    #[must_use]
    pub fn file(&self) -> File {
        self.module.file()
    }

    #[must_use]
    pub fn symbols(&self) -> &[TemplateSymbol] {
        &self.symbols
    }

    #[must_use]
    fn available_app(&self) -> Option<&PythonModuleName> {
        match self.status {
            TemplateLibraryStatus::Available => self.app.as_ref(),
            TemplateLibraryStatus::Builtin | TemplateLibraryStatus::Installed => None,
        }
    }
}

fn merge_symbols(symbols: Vec<TemplateSymbol>) -> Vec<TemplateSymbol> {
    let mut merged: Vec<TemplateSymbol> = Vec::new();
    for new_symbol in symbols {
        if let Some(existing) = merged
            .iter_mut()
            .find(|symbol| symbol.kind == new_symbol.kind && symbol.name == new_symbol.name)
        {
            if existing.doc.is_none() {
                existing.doc = new_symbol.doc;
            }

            if new_symbol.definition.rank() > existing.definition.rank() {
                existing.definition = new_symbol.definition;
            }

            continue;
        }

        merged.push(new_symbol);
    }

    merged.sort_by(|left, right| left.kind.cmp(&right.kind).then(left.name.cmp(&right.name)));
    merged
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TemplateSymbolAvailability {
    Builtin { module: PythonModuleName },
    RequiresLoad { load_name: LibraryName },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateSymbolCandidate {
    pub symbol: TemplateSymbol,
    pub availability: TemplateSymbolAvailability,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnknownSymbolOutcome {
    Suppressed,
    Available {
        app: PythonModuleName,
        load_name: LibraryName,
    },
    TrulyUnknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnknownLibraryOutcome {
    Suppressed,
    AvailableInApps {
        primary_app: PythonModuleName,
        apps: Vec<PythonModuleName>,
    },
    TrulyUnknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateLibraries {
    knowledge: StaticKnowledge,
    libraries: Vec<TemplateLibrary>,
    installed_by_name: BTreeMap<LibraryName, usize>,
    available_by_name: BTreeMap<LibraryName, Vec<usize>>,
}

impl Default for TemplateLibraries {
    fn default() -> Self {
        Self {
            knowledge: StaticKnowledge::Unknown,
            libraries: Vec::new(),
            installed_by_name: BTreeMap::new(),
            available_by_name: BTreeMap::new(),
        }
    }
}

impl TemplateLibraries {
    #[must_use]
    pub fn empty_ref() -> &'static Self {
        static EMPTY: std::sync::LazyLock<TemplateLibraries> =
            std::sync::LazyLock::new(TemplateLibraries::default);
        &EMPTY
    }

    #[must_use]
    pub(crate) fn from_libraries(
        knowledge: StaticKnowledge,
        libraries: Vec<TemplateLibrary>,
    ) -> Self {
        let mut inventory = Self::from_knowledge(knowledge);

        for library in libraries {
            inventory.insert_library(library);
        }

        inventory.sort_and_dedup_available();
        inventory
    }

    fn from_knowledge(knowledge: StaticKnowledge) -> Self {
        Self {
            knowledge,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn has_symbol_inventory(&self) -> bool {
        self.knowledge != StaticKnowledge::Unknown
    }

    #[must_use]
    pub fn inventory_is_complete(&self) -> bool {
        self.knowledge == StaticKnowledge::Known
    }

    #[must_use]
    pub fn inventory_may_omit_loaded_symbols(&self) -> bool {
        self.knowledge == StaticKnowledge::Partial
    }

    fn weaken_knowledge(&mut self, knowledge: StaticKnowledge) {
        self.knowledge = self.knowledge.weakened_by(knowledge);
    }

    fn demote_knowledge_to_partial(&mut self) {
        self.knowledge = self.knowledge.demoted_to_partial();
    }

    #[must_use]
    pub fn installed_library_count(&self) -> usize {
        self.builtin_libraries().count() + self.installed_libraries().count()
    }

    #[must_use]
    pub fn completion_library_names(&self) -> Vec<LibraryName> {
        self.installed_by_name.keys().cloned().collect()
    }

    #[must_use]
    pub fn template_symbol_candidates(
        &self,
        kind: TemplateSymbolKind,
    ) -> Vec<TemplateSymbolCandidate> {
        let mut candidates = Vec::new();
        let mut builtin_candidates = BTreeMap::new();

        for library in self.builtin_libraries() {
            for symbol in library.symbols() {
                if symbol.kind != kind {
                    continue;
                }

                builtin_candidates.insert(
                    symbol.name.clone(),
                    TemplateSymbolCandidate {
                        symbol: symbol.clone(),
                        availability: TemplateSymbolAvailability::Builtin {
                            module: library.module_name().clone(),
                        },
                    },
                );
            }
        }
        candidates.extend(builtin_candidates.into_values());

        for (name, library) in self.installed_libraries() {
            for symbol in library.symbols() {
                if symbol.kind != kind {
                    continue;
                }

                candidates.push(TemplateSymbolCandidate {
                    symbol: symbol.clone(),
                    availability: TemplateSymbolAvailability::RequiresLoad {
                        load_name: name.clone(),
                    },
                });
            }
        }

        candidates
    }

    #[must_use]
    pub fn installed_library(&self, name: &LibraryName) -> Option<&TemplateLibrary> {
        self.installed_by_name
            .get(name)
            .and_then(|index| self.libraries.get(*index))
    }

    #[must_use]
    pub fn installed_library_str(&self, name: &str) -> Option<&TemplateLibrary> {
        let name = LibraryName::parse(name).ok()?;
        self.installed_library(&name)
    }

    #[must_use]
    pub fn installed_library_module(&self, name: &LibraryName) -> Option<&PythonModuleName> {
        self.installed_library(name)
            .map(TemplateLibrary::module_name)
    }

    #[must_use]
    pub fn unknown_tag_outcome(&self, name: &str) -> UnknownSymbolOutcome {
        if !self.inventory_is_complete() {
            return UnknownSymbolOutcome::Suppressed;
        }

        let candidates = self.available_symbol_candidates(name, TemplateSymbolKind::Tag);
        unknown_symbol_candidate_outcome(&candidates)
    }

    #[must_use]
    pub fn unknown_filter_outcome(&self, name: &str) -> UnknownSymbolOutcome {
        if !self.inventory_is_complete() {
            return UnknownSymbolOutcome::Suppressed;
        }

        let candidates = self.available_symbol_candidates(name, TemplateSymbolKind::Filter);
        unknown_symbol_candidate_outcome(&candidates)
    }

    #[must_use]
    pub fn unknown_library_outcome(&self, name: &LibraryName) -> UnknownLibraryOutcome {
        if !self.inventory_is_complete() || self.installed_by_name.contains_key(name) {
            return UnknownLibraryOutcome::Suppressed;
        }

        let candidates = self.available_library_candidates(name);
        let Some(first) = candidates.first() else {
            return UnknownLibraryOutcome::TrulyUnknown;
        };
        let Some(primary_app) = first.available_app().cloned() else {
            return UnknownLibraryOutcome::TrulyUnknown;
        };
        let mut apps: Vec<_> = candidates
            .iter()
            .filter_map(|candidate| candidate.available_app().cloned())
            .collect();
        apps.dedup();

        UnknownLibraryOutcome::AvailableInApps { primary_app, apps }
    }

    pub fn active_libraries(&self) -> impl Iterator<Item = &TemplateLibrary> + '_ {
        let libraries = &self.libraries;
        self.installed_by_name
            .values()
            .copied()
            .chain(libraries.iter().enumerate().filter_map(|(index, library)| {
                (library.status == TemplateLibraryStatus::Builtin).then_some(index)
            }))
            .filter_map(|index| libraries.get(index))
    }

    fn builtin_libraries(&self) -> impl Iterator<Item = &TemplateLibrary> + '_ {
        self.libraries
            .iter()
            .filter(|library| library.status == TemplateLibraryStatus::Builtin)
    }

    fn installed_libraries(&self) -> impl Iterator<Item = (&LibraryName, &TemplateLibrary)> + '_ {
        self.installed_by_name
            .iter()
            .filter_map(|(name, index)| self.libraries.get(*index).map(|library| (name, library)))
    }

    #[must_use]
    fn available_library_candidates(&self, name: &LibraryName) -> Vec<&TemplateLibrary> {
        self.available_by_name
            .get(name)
            .into_iter()
            .flatten()
            .filter_map(|index| self.libraries.get(*index))
            .collect()
    }

    fn available_symbol_candidates(
        &self,
        symbol_name: &str,
        kind: TemplateSymbolKind,
    ) -> Vec<&TemplateLibrary> {
        let mut candidates: Vec<_> = self
            .available_by_name
            .values()
            .flatten()
            .filter_map(|index| self.libraries.get(*index))
            .filter(|library| {
                library
                    .symbols()
                    .iter()
                    .any(|symbol| symbol.kind == kind && symbol.name.as_str() == symbol_name)
            })
            .collect();
        candidates.sort_by(|left, right| cmp_available_libraries(left, right));
        candidates
    }

    fn insert_library(&mut self, library: TemplateLibrary) {
        match library.status {
            TemplateLibraryStatus::Builtin => self.libraries.push(library),
            TemplateLibraryStatus::Installed => {
                let name = library
                    .load_name
                    .clone()
                    .expect("installed libraries should carry a load name");
                if let Some(index) = self.installed_by_name.get(&name).copied() {
                    self.libraries[index] = library;
                } else {
                    let index = self.libraries.len();
                    self.libraries.push(library);
                    self.installed_by_name.insert(name, index);
                }
            }
            TemplateLibraryStatus::Available => {
                let name = library
                    .load_name
                    .clone()
                    .expect("available libraries should carry a load name");
                let app = library
                    .app
                    .clone()
                    .expect("available libraries should carry an app");
                let module_name = library.module_name().clone();
                if let Some(existing_index) = self.libraries.iter().position(|existing| {
                    existing.status == TemplateLibraryStatus::Available
                        && existing.load_name() == Some(&name)
                        && existing.app.as_ref() == Some(&app)
                        && existing.module_name() == &module_name
                }) {
                    let indexes = self.available_by_name.entry(name).or_default();
                    if !indexes.contains(&existing_index) {
                        indexes.push(existing_index);
                    }
                    return;
                }

                let index = self.libraries.len();
                self.libraries.push(library);
                self.available_by_name.entry(name).or_default().push(index);
            }
        }
    }

    fn insert_available_candidates(
        &mut self,
        db: &dyn ProjectDb,
        project: Project,
        installed_template_library_modules: &BTreeSet<PythonModuleName>,
    ) {
        let mut excluded_modules: BTreeSet<_> = self
            .installed_libraries()
            .map(|(_name, library)| library.module_name().clone())
            .chain(
                self.builtin_libraries()
                    .map(TemplateLibrary::module_name)
                    .cloned(),
            )
            .collect();

        excluded_modules.extend(installed_template_library_modules.iter().cloned());

        for candidate in templatetag_candidates(db, project).iter().cloned() {
            if excluded_modules.contains(&candidate.module) {
                continue;
            }

            let analysis = TemplateLibraryAnalysis::from_file(db, candidate.file);
            if !analysis.defines_library && analysis.symbols.is_empty() {
                continue;
            }

            self.insert_library(TemplateLibrary::available(
                candidate.name.clone(),
                candidate.app.clone(),
                candidate.into_python_module(),
                analysis.symbols,
            ));
        }

        self.sort_and_dedup_available();
    }

    fn sort_and_dedup_available(&mut self) {
        let libraries = &self.libraries;
        for indexes in self.available_by_name.values_mut() {
            indexes.sort_by(
                |left, right| match (libraries.get(*left), libraries.get(*right)) {
                    (Some(left), Some(right)) => cmp_available_libraries(left, right),
                    (None, _) | (_, None) => Ordering::Equal,
                },
            );
            indexes.dedup_by(
                |left, right| match (libraries.get(*left), libraries.get(*right)) {
                    (Some(left), Some(right)) => same_available_library(left, right),
                    (None, _) | (_, None) => false,
                },
            );
        }
    }
}

#[salsa::tracked(returns(ref))]
pub fn template_libraries(db: &dyn ProjectDb, project: Project) -> TemplateLibraries {
    project.touch_search_path_roots(db);

    if settings_module_file(db, project).is_none() {
        return TemplateLibraries::default();
    }

    let settings = django_settings(db, project);

    let mut libraries =
        TemplateLibraries::from_knowledge(match settings.installed_apps.knowledge {
            StaticKnowledge::Known => StaticKnowledge::Known,
            StaticKnowledge::Partial | StaticKnowledge::Unknown => StaticKnowledge::Partial,
        });

    if settings.templates.knowledge != StaticKnowledge::Known {
        libraries.demote_knowledge_to_partial();
    }

    let mut installed_template_library_modules = BTreeSet::new();

    let (knowledge, discovered_libraries) = templatetag_package_libraries(db, project, "django");
    libraries.weaken_knowledge(knowledge);
    for (load_name, module, symbols) in discovered_libraries {
        installed_template_library_modules.insert(module.name().clone());
        libraries.insert_library(TemplateLibrary::installed(load_name, module, symbols));
    }

    if settings.installed_apps.knowledge != StaticKnowledge::Unknown {
        for installed_app in &settings.installed_apps.values {
            let (knowledge, discovered_libraries) = templatetag_package_libraries(
                db,
                project,
                guess_package_module_from_installed_app_entry(installed_app),
            );
            libraries.weaken_knowledge(knowledge);
            for (load_name, module, symbols) in discovered_libraries {
                installed_template_library_modules.insert(module.name().clone());
                libraries.insert_library(TemplateLibrary::installed(load_name, module, symbols));
            }
        }
    }

    let backend_count = settings.templates.backends.len();
    for backend in settings
        .templates
        .backends
        .iter()
        .filter(|backend| backend.is_django_templates_backend(backend_count))
    {
        libraries.weaken_knowledge(backend.knowledge);

        for (load_name, module_name) in &backend.libraries {
            let Ok(load_name) = LibraryName::parse(load_name) else {
                libraries.demote_knowledge_to_partial();
                continue;
            };
            let (knowledge, library) = library_from_module_name(db, project, module_name);
            libraries.weaken_knowledge(knowledge);
            let Some((module, symbols)) = library else {
                continue;
            };
            libraries.insert_library(TemplateLibrary::installed(load_name, module, symbols));
        }

        for module_name in DEFAULT_TEMPLATE_BUILTINS.iter().copied() {
            let (knowledge, library) = library_from_module_name(db, project, module_name);
            libraries.weaken_knowledge(knowledge);
            let Some((module, symbols)) = library else {
                continue;
            };
            libraries.insert_library(TemplateLibrary::builtin(module, symbols));
        }

        for module_name in backend.builtins.iter().map(String::as_str) {
            let (knowledge, library) = library_from_module_name(db, project, module_name);
            libraries.weaken_knowledge(knowledge);
            let Some((module, symbols)) = library else {
                continue;
            };
            libraries.insert_library(TemplateLibrary::builtin(module, symbols));
        }
    }

    libraries.insert_available_candidates(db, project, &installed_template_library_modules);
    libraries
}

fn templatetag_package_libraries(
    db: &dyn ProjectDb,
    project: Project,
    package_module: &str,
) -> (
    StaticKnowledge,
    Vec<(LibraryName, PythonModule, Vec<TemplateSymbol>)>,
) {
    let (knowledge, candidates) =
        templatetag_package_candidates(db, project, package_module).into_parts();
    let mut libraries = Vec::new();

    for candidate in candidates {
        let analysis = TemplateLibraryAnalysis::from_file(db, candidate.file);
        if !analysis.defines_library && analysis.symbols.is_empty() {
            continue;
        }

        libraries.push((
            candidate.name.clone(),
            candidate.into_python_module(),
            analysis.symbols,
        ));
    }

    (knowledge, libraries)
}

fn library_from_module_name(
    db: &dyn ProjectDb,
    project: Project,
    module_name: &str,
) -> (StaticKnowledge, Option<(PythonModule, Vec<TemplateSymbol>)>) {
    let Ok(Some(module)) = PythonResolver::new(db, project).module_from_str(module_name) else {
        return (StaticKnowledge::Partial, None);
    };

    let analysis = TemplateLibraryAnalysis::from_file(db, module.file());
    if !analysis.defines_library && analysis.symbols.is_empty() {
        return (StaticKnowledge::Partial, None);
    }

    (StaticKnowledge::Known, Some((module, analysis.symbols)))
}

fn unknown_symbol_candidate_outcome(candidates: &[&TemplateLibrary]) -> UnknownSymbolOutcome {
    if let Some(candidate) = candidates.first() {
        return UnknownSymbolOutcome::Available {
            app: candidate
                .available_app()
                .expect("available candidates should carry an app")
                .clone(),
            load_name: candidate
                .load_name()
                .expect("available candidates should carry a load name")
                .clone(),
        };
    }

    UnknownSymbolOutcome::TrulyUnknown
}

fn cmp_available_libraries(left: &TemplateLibrary, right: &TemplateLibrary) -> Ordering {
    left.available_app()
        .cmp(&right.available_app())
        .then_with(|| left.load_name().cmp(&right.load_name()))
        .then_with(|| left.module_name_str().cmp(right.module_name_str()))
}

fn same_available_library(left: &TemplateLibrary, right: &TemplateLibrary) -> bool {
    let (Some(left_app), Some(right_app)) = (left.available_app(), right.available_app()) else {
        return false;
    };

    left_app == right_app && left.module_name_str() == right.module_name_str()
}

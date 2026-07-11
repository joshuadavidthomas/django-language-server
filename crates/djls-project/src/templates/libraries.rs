use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::collections::BTreeSet;

use djls_source::File;

use super::candidates::templatetag_candidates;
use super::candidates::templatetag_package_candidates;
use super::installed_app_package_module;
use super::names::LibraryName;
use super::registrations::TemplateLibraryAnalysis;
use super::registrations::TemplateLibrarySource;
use super::symbols::TemplateSymbol;
use super::symbols::TemplateSymbolKind;
use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonModule;
use crate::python::PythonModuleName;
use crate::settings::django_settings;
use crate::settings::settings_module_file;
use crate::settings::types::TemplateBackend;

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
pub enum LoadableLibraryLookup<'a> {
    Found(&'a TemplateLibrary),
    Ambiguous(Vec<&'a TemplateLibrary>),
    Inconclusive(Vec<&'a TemplateLibrary>),
    Absent,
}

impl<'a> LoadableLibraryLookup<'a> {
    /// Return the library only when every feasible configuration agrees.
    #[must_use]
    pub fn found(self) -> Option<&'a TemplateLibrary> {
        match self {
            Self::Found(library) => Some(library),
            Self::Ambiguous(_) | Self::Inconclusive(_) | Self::Absent => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TemplateSymbolLookup {
    FoundInApp {
        app: PythonModuleName,
        load_name: LibraryName,
    },
    Absent,
    Inconclusive,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MissingLibraryLookup {
    FoundInApps {
        primary_app: PythonModuleName,
        apps: Vec<PythonModuleName>,
    },
    Absent,
    Inconclusive,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TemplateLibraryIssue {
    OpenSettings,
    Discovery,
    ConfiguredAlias(LibraryName),
    Source(Option<LibraryName>),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TemplateLibraryConfigurations {
    known: Vec<TemplateLibraryConfiguration>,
    open: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TemplateLibraryConfiguration {
    backends: Vec<TemplateBackendLibraries>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TemplateBackendLibraries {
    loadable_by_name: BTreeMap<LibraryName, usize>,
    builtin_indices: Vec<usize>,
}

type TestingBackendConfiguration = (Vec<(LibraryName, PythonModuleName)>, Vec<PythonModuleName>);
type DiscoveredLibrary = (LibraryName, PythonModule, Vec<TemplateSymbol>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateLibraries {
    libraries: Vec<TemplateLibrary>,
    installed_by_name: BTreeMap<LibraryName, usize>,
    configured_claims: BTreeSet<LibraryName>,
    configurations: TemplateLibraryConfigurations,
    available_by_name: BTreeMap<LibraryName, Vec<usize>>,
    issues: Vec<TemplateLibraryIssue>,
}

impl Default for TemplateLibraries {
    fn default() -> Self {
        Self {
            libraries: Vec::new(),
            installed_by_name: BTreeMap::new(),
            configured_claims: BTreeSet::new(),
            configurations: TemplateLibraryConfigurations {
                known: Vec::new(),
                open: true,
            },
            available_by_name: BTreeMap::new(),
            issues: vec![TemplateLibraryIssue::OpenSettings],
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
    pub(crate) fn from_libraries(open: bool, libraries: Vec<TemplateLibrary>) -> Self {
        let mut inventory = Self {
            libraries: Vec::new(),
            installed_by_name: BTreeMap::new(),
            configured_claims: BTreeSet::new(),
            configurations: TemplateLibraryConfigurations {
                known: vec![TemplateLibraryConfiguration {
                    backends: vec![TemplateBackendLibraries::default()],
                }],
                open,
            },
            available_by_name: BTreeMap::new(),
            issues: if open {
                vec![TemplateLibraryIssue::OpenSettings]
            } else {
                Vec::new()
            },
        };

        for library in libraries {
            inventory.insert_library(library);
        }
        inventory.configured_claims = inventory.installed_by_name.keys().cloned().collect();
        inventory.rebuild_configuration();
        inventory.sort_and_dedup_available();
        inventory
    }

    pub(crate) fn set_testing_configurations(
        &mut self,
        configurations: Vec<Vec<TestingBackendConfiguration>>,
    ) {
        if configurations.is_empty() {
            self.configurations.open = true;
        }
        self.configurations.known = configurations
            .into_iter()
            .map(|backends| TemplateLibraryConfiguration {
                backends: backends
                    .into_iter()
                    .map(|(loadable, builtins)| TemplateBackendLibraries {
                        loadable_by_name: loadable
                            .into_iter()
                            .filter_map(|(name, module)| {
                                self.libraries
                                    .iter()
                                    .enumerate()
                                    .rev()
                                    .find(|(_index, library)| {
                                        library.status == TemplateLibraryStatus::Installed
                                            && library.load_name() == Some(&name)
                                            && library.module_name() == &module
                                    })
                                    .map(|(index, _library)| (name, index))
                            })
                            .collect(),
                        builtin_indices: builtins
                            .into_iter()
                            .filter_map(|module| {
                                self.libraries
                                    .iter()
                                    .enumerate()
                                    .find(|(_index, library)| {
                                        library.status == TemplateLibraryStatus::Builtin
                                            && library.module_name() == &module
                                    })
                                    .map(|(index, _library)| index)
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect();
    }

    fn rebuild_configuration(&mut self) {
        let backend = TemplateBackendLibraries {
            loadable_by_name: self.installed_by_name.clone(),
            builtin_indices: self
                .libraries
                .iter()
                .enumerate()
                .filter_map(|(index, library)| {
                    (library.status == TemplateLibraryStatus::Builtin).then_some(index)
                })
                .collect(),
        };
        self.configurations.known = vec![TemplateLibraryConfiguration {
            backends: vec![backend],
        }];
    }

    #[must_use]
    pub fn installed_library_count(&self) -> usize {
        self.builtin_libraries().count() + self.installed_libraries().count()
    }

    #[must_use]
    pub fn completion_library_names(&self) -> Vec<LibraryName> {
        self.installed_by_name
            .keys()
            .chain(&self.configured_claims)
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
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
    pub fn loadable_library(&self, name: &LibraryName) -> LoadableLibraryLookup<'_> {
        let mut outcomes = Vec::new();
        let mut indexes = Vec::new();
        for configuration in &self.configurations.known {
            let mut configuration_matches = Vec::new();
            let mut absent = configuration.backends.is_empty();
            for backend in &configuration.backends {
                match backend.loadable_by_name.get(name).copied() {
                    Some(index) if !configuration_matches.contains(&index) => {
                        configuration_matches.push(index);
                        if !indexes.contains(&index) {
                            indexes.push(index);
                        }
                    }
                    Some(_) => {}
                    None => absent = true,
                }
            }
            outcomes.push((configuration_matches, absent));
        }

        indexes.sort_unstable();
        let records: Vec<_> = indexes
            .iter()
            .filter_map(|index| self.libraries.get(*index))
            .collect();
        let unanimous_index = outcomes
            .first()
            .and_then(|(matches, absent)| (!*absent && matches.len() == 1).then(|| matches[0]));
        let unanimous = unanimous_index.is_some_and(|index| {
            outcomes
                .iter()
                .all(|(matches, absent)| !*absent && matches.as_slice() == [index])
        });

        if self.configurations.open
            || self
                .issues
                .iter()
                .any(|issue| matches!(issue, TemplateLibraryIssue::ConfiguredAlias(alias) if alias == name))
            || (records.is_empty()
                && !self.configured_claims.contains(name)
                && self.issues.iter().any(|issue| match issue {
                    TemplateLibraryIssue::Discovery => true,
                    TemplateLibraryIssue::Source(Some(source_name)) => source_name == name,
                    TemplateLibraryIssue::OpenSettings
                    | TemplateLibraryIssue::ConfiguredAlias(_)
                    | TemplateLibraryIssue::Source(None) => false,
                }))
        {
            return LoadableLibraryLookup::Inconclusive(records);
        }
        if unanimous && let Some(library) = records.first() {
            return LoadableLibraryLookup::Found(library);
        }
        if outcomes
            .iter()
            .all(|(matches, absent)| matches.is_empty() && *absent)
        {
            LoadableLibraryLookup::Absent
        } else {
            LoadableLibraryLookup::Ambiguous(records)
        }
    }

    #[must_use]
    pub fn loadable_library_str(&self, name: &str) -> LoadableLibraryLookup<'_> {
        match LibraryName::parse(name) {
            Ok(name) => self.loadable_library(&name),
            Err(_) => LoadableLibraryLookup::Absent,
        }
    }

    #[must_use]
    pub fn template_symbol_lookup(
        &self,
        name: &str,
        kind: TemplateSymbolKind,
    ) -> TemplateSymbolLookup {
        let candidates = self.available_symbol_candidates(name, kind);
        if let Some(candidate) = candidates.first()
            && let (Some(app), Some(load_name)) = (candidate.available_app(), candidate.load_name())
        {
            return TemplateSymbolLookup::FoundInApp {
                app: app.clone(),
                load_name: load_name.clone(),
            };
        }
        if self.configurations.open || !self.issues.is_empty() {
            TemplateSymbolLookup::Inconclusive
        } else {
            TemplateSymbolLookup::Absent
        }
    }

    #[must_use]
    pub fn missing_library_lookup(&self, name: &LibraryName) -> MissingLibraryLookup {
        match self.loadable_library(name) {
            LoadableLibraryLookup::Found(_) | LoadableLibraryLookup::Ambiguous(_) => {
                return MissingLibraryLookup::Inconclusive;
            }
            LoadableLibraryLookup::Inconclusive(candidates)
                if !candidates.is_empty()
                    || self.issues.iter().any(|issue| {
                        matches!(issue, TemplateLibraryIssue::ConfiguredAlias(alias) if alias == name)
                    }) =>
            {
                return MissingLibraryLookup::Inconclusive;
            }
            LoadableLibraryLookup::Inconclusive(_) | LoadableLibraryLookup::Absent => {}
        }
        let candidates = self.available_library_candidates(name);
        if let Some(first) = candidates.first()
            && let Some(primary_app) = first.available_app().cloned()
        {
            let mut apps: Vec<_> = candidates
                .iter()
                .filter_map(|candidate| candidate.available_app().cloned())
                .collect();
            apps.dedup();
            return MissingLibraryLookup::FoundInApps { primary_app, apps };
        }
        if self.configurations.open
            || self.issues.iter().any(|issue| match issue {
                TemplateLibraryIssue::Discovery | TemplateLibraryIssue::OpenSettings => true,
                TemplateLibraryIssue::ConfiguredAlias(alias) => alias == name,
                TemplateLibraryIssue::Source(Some(source_name)) => source_name == name,
                TemplateLibraryIssue::Source(None) => false,
            })
        {
            MissingLibraryLookup::Inconclusive
        } else {
            MissingLibraryLookup::Absent
        }
    }

    pub fn resolved_libraries(&self) -> impl Iterator<Item = &TemplateLibrary> + '_ {
        let mut indexes = Vec::new();
        for configuration in &self.configurations.known {
            for backend in &configuration.backends {
                for index in backend
                    .loadable_by_name
                    .values()
                    .copied()
                    .chain(backend.builtin_indices.iter().copied())
                {
                    if !indexes.contains(&index) {
                        indexes.push(index);
                    }
                }
            }
        }
        indexes
            .into_iter()
            .filter_map(|index| self.libraries.get(index))
    }

    pub fn definitely_loaded_libraries(&self) -> impl Iterator<Item = &TemplateLibrary> + '_ {
        let closed_single_backend = !self.configurations.open
            && self.configurations.known.len() == 1
            && self.configurations.known[0].backends.len() == 1;
        self.resolved_libraries()
            .filter(move |_| closed_single_backend)
    }

    fn builtin_libraries(&self) -> impl Iterator<Item = &TemplateLibrary> + '_ {
        self.resolved_libraries()
            .filter(|library| library.status == TemplateLibraryStatus::Builtin)
    }

    fn installed_libraries(&self) -> impl Iterator<Item = (&LibraryName, &TemplateLibrary)> + '_ {
        self.resolved_libraries().filter_map(|library| {
            (library.status == TemplateLibraryStatus::Installed)
                .then(|| library.load_name().map(|name| (name, library)))
                .flatten()
        })
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

    fn insert_library(&mut self, library: TemplateLibrary) -> usize {
        match library.status {
            TemplateLibraryStatus::Builtin => {
                let index = self.libraries.len();
                self.libraries.push(library);
                index
            }
            TemplateLibraryStatus::Installed => {
                let name = library
                    .load_name
                    .clone()
                    .expect("installed libraries should carry a load name");
                let index = self.libraries.len();
                self.libraries.push(library);
                self.installed_by_name.insert(name, index);
                index
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
                    return existing_index;
                }

                let index = self.libraries.len();
                self.libraries.push(library);
                self.available_by_name.entry(name).or_default().push(index);
                index
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

        let candidates = templatetag_candidates(db, project);
        if candidates.has_omissions() {
            self.issues.push(TemplateLibraryIssue::Discovery);
        }
        for candidate in candidates.candidates().iter().cloned() {
            if excluded_modules.contains(candidate.module.name()) {
                continue;
            }

            match TemplateLibraryAnalysis::from_file(db, candidate.module.file()) {
                TemplateLibraryAnalysis::Failed => {
                    self.issues
                        .push(TemplateLibraryIssue::Source(Some(candidate.name.clone())));
                }
                TemplateLibraryAnalysis::ParsedNotLibrary { source } => {
                    if source == TemplateLibrarySource::Recovered {
                        self.issues
                            .push(TemplateLibraryIssue::Source(Some(candidate.name.clone())));
                    }
                }
                TemplateLibraryAnalysis::Library { symbols, source } => {
                    if source == TemplateLibrarySource::Recovered {
                        self.issues
                            .push(TemplateLibraryIssue::Source(Some(candidate.name.clone())));
                    }
                    self.insert_library(TemplateLibrary::available(
                        candidate.name.clone(),
                        candidate.app.clone(),
                        candidate.into_python_module(),
                        symbols,
                    ));
                }
            }
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
    let open =
        !settings.installed_apps.is_fully_extracted() || !settings.templates.is_fully_extracted();
    let mut libraries = TemplateLibraries::from_libraries(open, Vec::new());
    let mut installed_template_library_modules = BTreeSet::new();

    let django_module = PythonModuleName::parse("django").expect("django is a valid module name");
    let (discovered, issues) = templatetag_package_libraries(db, project, &django_module);
    libraries.issues.extend(issues);
    insert_installed_libraries(
        &mut libraries,
        &mut installed_template_library_modules,
        discovered,
    );

    for installed_app in &settings.installed_apps.values {
        let Some(package_module) = installed_app_package_module(db, project, installed_app) else {
            libraries.issues.push(TemplateLibraryIssue::Discovery);
            continue;
        };
        let (discovered, issues) = templatetag_package_libraries(db, project, &package_module);
        libraries.issues.extend(issues);
        insert_installed_libraries(
            &mut libraries,
            &mut installed_template_library_modules,
            discovered,
        );
    }

    let app_libraries = libraries.installed_by_name.clone();
    let mut backend_configurations = Vec::new();
    for backend in settings
        .templates
        .backends
        .iter()
        .filter(|backend| backend.is_django_templates_backend())
    {
        backend_configurations.push(insert_backend_libraries(
            db,
            project,
            backend,
            &app_libraries,
            &mut libraries,
        ));
    }

    if backend_configurations.is_empty() {
        backend_configurations.push(TemplateBackendLibraries {
            loadable_by_name: app_libraries,
            builtin_indices: Vec::new(),
        });
    }
    libraries.configurations.known = vec![TemplateLibraryConfiguration {
        backends: backend_configurations,
    }];
    libraries.insert_available_candidates(db, project, &installed_template_library_modules);
    libraries
}

fn insert_backend_libraries(
    db: &dyn ProjectDb,
    project: Project,
    backend: &TemplateBackend,
    app_libraries: &BTreeMap<LibraryName, usize>,
    libraries: &mut TemplateLibraries,
) -> TemplateBackendLibraries {
    if !backend.is_fully_extracted() {
        libraries.configurations.open = true;
        libraries.issues.push(TemplateLibraryIssue::OpenSettings);
    }

    let mut result = TemplateBackendLibraries {
        loadable_by_name: app_libraries.clone(),
        builtin_indices: Vec::new(),
    };
    for (load_name, module_name) in &backend.libraries {
        let Ok(load_name) = LibraryName::parse(load_name) else {
            libraries.issues.push(TemplateLibraryIssue::OpenSettings);
            continue;
        };
        libraries.configured_claims.insert(load_name.clone());
        let Some((module, symbols, source)) =
            library_from_module_name(db, project, module_name.clone())
        else {
            libraries
                .issues
                .push(TemplateLibraryIssue::ConfiguredAlias(load_name));
            continue;
        };
        if source == TemplateLibrarySource::Recovered {
            libraries
                .issues
                .push(TemplateLibraryIssue::Source(Some(load_name.clone())));
        }
        let index = libraries.insert_library(TemplateLibrary::installed(
            load_name.clone(),
            module,
            symbols,
        ));
        result.loadable_by_name.insert(load_name, index);
    }

    let builtins = DEFAULT_TEMPLATE_BUILTINS
        .iter()
        .map(|name| PythonModuleName::parse(name).expect("default builtin is a valid module name"))
        .chain(backend.builtins.iter().cloned());
    for module_name in builtins {
        match library_from_module_name(db, project, module_name) {
            Some((module, symbols, source)) => {
                if source == TemplateLibrarySource::Recovered {
                    libraries.issues.push(TemplateLibraryIssue::Source(None));
                }
                let index = libraries.insert_library(TemplateLibrary::builtin(module, symbols));
                result.builtin_indices.push(index);
            }
            None => libraries.issues.push(TemplateLibraryIssue::Source(None)),
        }
    }

    result
}

fn insert_installed_libraries(
    libraries: &mut TemplateLibraries,
    installed_modules: &mut BTreeSet<PythonModuleName>,
    discovered: Vec<(LibraryName, PythonModule, Vec<TemplateSymbol>)>,
) {
    for (load_name, module, symbols) in discovered {
        installed_modules.insert(module.name().clone());
        libraries.insert_library(TemplateLibrary::installed(load_name, module, symbols));
    }
}

fn templatetag_package_libraries(
    db: &dyn ProjectDb,
    project: Project,
    package_module: &PythonModuleName,
) -> (Vec<DiscoveredLibrary>, Vec<TemplateLibraryIssue>) {
    let (candidates, candidate_issues) =
        templatetag_package_candidates(db, project, package_module).into_parts();
    let mut issues = candidate_issues
        .into_iter()
        .map(|_| TemplateLibraryIssue::Discovery)
        .collect::<Vec<_>>();
    let mut libraries = Vec::new();

    for candidate in candidates {
        match TemplateLibraryAnalysis::from_file(db, candidate.module.file()) {
            TemplateLibraryAnalysis::Failed => {
                issues.push(TemplateLibraryIssue::Source(Some(candidate.name.clone())));
            }
            TemplateLibraryAnalysis::ParsedNotLibrary { source } => {
                if source == TemplateLibrarySource::Recovered {
                    issues.push(TemplateLibraryIssue::Source(Some(candidate.name.clone())));
                }
            }
            TemplateLibraryAnalysis::Library { symbols, source } => {
                if source == TemplateLibrarySource::Recovered {
                    issues.push(TemplateLibraryIssue::Source(Some(candidate.name.clone())));
                }
                libraries.push((
                    candidate.name.clone(),
                    candidate.into_python_module(),
                    symbols,
                ));
            }
        }
    }

    (libraries, issues)
}

fn library_from_module_name(
    db: &dyn ProjectDb,
    project: Project,
    module_name: PythonModuleName,
) -> Option<(PythonModule, Vec<TemplateSymbol>, TemplateLibrarySource)> {
    let module = PythonModule::resolve(db, project, module_name)?;

    match TemplateLibraryAnalysis::from_file(db, module.file()) {
        TemplateLibraryAnalysis::Failed | TemplateLibraryAnalysis::ParsedNotLibrary { .. } => None,
        TemplateLibraryAnalysis::Library { symbols, source } => Some((module, symbols, source)),
    }
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

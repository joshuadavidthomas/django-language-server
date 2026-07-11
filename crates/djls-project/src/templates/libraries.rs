use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::collections::BTreeSet;

use djls_source::File;

use super::candidates::templatetag_candidates;
use super::candidates::templatetag_candidates_in_package;
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

#[derive(Clone, Debug, PartialEq, Eq)]
enum TemplateLibraryKind {
    Builtin,
    Installed {
        load_name: LibraryName,
    },
    Available {
        load_name: LibraryName,
        app: PythonModuleName,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateLibrary {
    module: PythonModule,
    kind: TemplateLibraryKind,
    symbols: Vec<TemplateSymbol>,
}

impl TemplateLibrary {
    #[must_use]
    pub(crate) fn builtin(module: PythonModule, symbols: Vec<TemplateSymbol>) -> Self {
        Self {
            module,
            kind: TemplateLibraryKind::Builtin,
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
            module,
            kind: TemplateLibraryKind::Installed { load_name },
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
            module,
            kind: TemplateLibraryKind::Available { load_name, app },
            symbols: merge_symbols(symbols),
        }
    }

    #[must_use]
    pub fn load_name(&self) -> Option<&LibraryName> {
        match &self.kind {
            TemplateLibraryKind::Builtin => None,
            TemplateLibraryKind::Installed { load_name }
            | TemplateLibraryKind::Available { load_name, .. } => Some(load_name),
        }
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
        match &self.kind {
            TemplateLibraryKind::Available { app, .. } => Some(app),
            TemplateLibraryKind::Builtin | TemplateLibraryKind::Installed { .. } => None,
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
pub struct AvailableAppCandidates(Vec<PythonModuleName>);

impl AvailableAppCandidates {
    /// Return the first available app in deterministic lookup order.
    ///
    /// # Panics
    ///
    /// Panics only if the private non-empty candidate invariant is violated.
    #[must_use]
    pub fn primary(&self) -> &PythonModuleName {
        self.0
            .first()
            .expect("available app candidates should be non-empty")
    }

    #[must_use]
    pub fn as_slice(&self) -> &[PythonModuleName] {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MissingLibraryLookup {
    FoundInApps(AvailableAppCandidates),
    Absent,
    Inconclusive,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TemplateLibraryIssue {
    Discovery,
    NamedSource(LibraryName),
    BuiltinSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TemplateConfigurationOmission {
    Settings,
    Backend,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TemplateLibraryConfigurations {
    Exhaustive(Vec<Vec<TemplateBackendLibraries>>),
    WithOmissions {
        known: Vec<Vec<TemplateBackendLibraries>>,
        omissions: Vec<TemplateConfigurationOmission>,
    },
}

impl TemplateLibraryConfigurations {
    fn known(&self) -> &[Vec<TemplateBackendLibraries>] {
        match self {
            Self::Exhaustive(known) | Self::WithOmissions { known, .. } => known,
        }
    }

    const fn has_omissions(&self) -> bool {
        matches!(self, Self::WithOmissions { .. })
    }

    fn replace_known(&mut self, known: Vec<Vec<TemplateBackendLibraries>>) {
        match self {
            Self::Exhaustive(_) => *self = Self::Exhaustive(known),
            Self::WithOmissions {
                known: existing, ..
            } => *existing = known,
        }
    }

    fn omit(&mut self, omission: TemplateConfigurationOmission) {
        match self {
            Self::Exhaustive(known) => {
                *self = Self::WithOmissions {
                    known: std::mem::take(known),
                    omissions: vec![omission],
                };
            }
            Self::WithOmissions { omissions, .. } if !omissions.contains(&omission) => {
                omissions.push(omission);
            }
            Self::WithOmissions { .. } => {}
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TemplateLibraryReference {
    Resolved(usize),
    Unresolved,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TemplateBackendLibraries {
    loadable_by_name: BTreeMap<LibraryName, TemplateLibraryReference>,
    builtin_indices: Vec<usize>,
}

type TestingBackendConfiguration = (Vec<(LibraryName, PythonModuleName)>, Vec<PythonModuleName>);
type DiscoveredLibrary = (LibraryName, PythonModule, Vec<TemplateSymbol>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateLibraries {
    libraries: Vec<TemplateLibrary>,
    installed_by_name: BTreeMap<LibraryName, usize>,
    configurations: TemplateLibraryConfigurations,
    available_by_name: BTreeMap<LibraryName, Vec<usize>>,
    issues: Vec<TemplateLibraryIssue>,
}

impl Default for TemplateLibraries {
    fn default() -> Self {
        Self {
            libraries: Vec::new(),
            installed_by_name: BTreeMap::new(),
            configurations: TemplateLibraryConfigurations::WithOmissions {
                known: Vec::new(),
                omissions: vec![TemplateConfigurationOmission::Settings],
            },
            available_by_name: BTreeMap::new(),
            issues: Vec::new(),
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
            configurations: if open {
                TemplateLibraryConfigurations::WithOmissions {
                    known: vec![vec![TemplateBackendLibraries::default()]],
                    omissions: vec![TemplateConfigurationOmission::Settings],
                }
            } else {
                TemplateLibraryConfigurations::Exhaustive(vec![vec![
                    TemplateBackendLibraries::default(),
                ]])
            },
            available_by_name: BTreeMap::new(),
            issues: Vec::new(),
        };

        for library in libraries {
            inventory.insert_library(library);
        }
        inventory.rebuild_configuration();
        inventory.sort_and_dedup_available();
        inventory
    }

    pub(crate) fn set_testing_configurations(
        &mut self,
        configurations: Vec<Vec<TestingBackendConfiguration>>,
    ) {
        let known = configurations
            .into_iter()
            .map(|backends| {
                backends
                    .into_iter()
                    .map(|(loadable, builtins)| TemplateBackendLibraries {
                        loadable_by_name: loadable
                            .into_iter()
                            .map(|(name, module)| {
                                let (index, _library) = self
                                    .libraries
                                    .iter()
                                    .enumerate()
                                    .rev()
                                    .find(|(_index, library)| {
                                        matches!(
                                            &library.kind,
                                            TemplateLibraryKind::Installed { load_name }
                                                if load_name == &name
                                        ) && library.module_name() == &module
                                    })
                                    .unwrap_or_else(|| {
                                        panic!(
                                            "configured test library {name} should resolve to {module}"
                                        )
                                    });
                                (name, TemplateLibraryReference::Resolved(index))
                            })
                            .collect(),
                        builtin_indices: builtins
                            .into_iter()
                            .map(|module| {
                                self.libraries
                                    .iter()
                                    .enumerate()
                                    .find(|(_index, library)| {
                                        matches!(&library.kind, TemplateLibraryKind::Builtin)
                                            && library.module_name() == &module
                                    }).map_or_else(|| {
                                        panic!("configured test builtin should resolve to {module}")
                                    }, |(index, _library)| index)
                            })
                            .collect(),
                    })
                    .collect()
            })
            .collect();
        self.configurations.replace_known(known);
    }

    fn rebuild_configuration(&mut self) {
        let backend = TemplateBackendLibraries {
            loadable_by_name: self
                .installed_by_name
                .iter()
                .map(|(name, index)| (name.clone(), TemplateLibraryReference::Resolved(*index)))
                .collect(),
            builtin_indices: self
                .libraries
                .iter()
                .enumerate()
                .filter_map(|(index, library)| {
                    matches!(&library.kind, TemplateLibraryKind::Builtin).then_some(index)
                })
                .collect(),
        };
        self.configurations.replace_known(vec![vec![backend]]);
    }

    #[must_use]
    pub fn installed_library_count(&self) -> usize {
        self.builtin_libraries().count() + self.installed_libraries().count()
    }

    #[must_use]
    pub fn completion_library_names(&self) -> Vec<LibraryName> {
        self.configurations
            .known()
            .iter()
            .flatten()
            .flat_map(|backend| backend.loadable_by_name.keys())
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
        let mut unresolved = false;
        for configuration in self.configurations.known() {
            let mut configuration_matches = Vec::new();
            let mut absent = configuration.is_empty();
            for backend in configuration {
                match backend.loadable_by_name.get(name).copied() {
                    Some(TemplateLibraryReference::Resolved(index))
                        if !configuration_matches.contains(&index) =>
                    {
                        configuration_matches.push(index);
                        if !indexes.contains(&index) {
                            indexes.push(index);
                        }
                    }
                    Some(TemplateLibraryReference::Resolved(_)) => {}
                    Some(TemplateLibraryReference::Unresolved) => unresolved = true,
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

        if self.configurations.has_omissions()
            || unresolved
            || (records.is_empty()
                && self.issues.iter().any(|issue| match issue {
                    TemplateLibraryIssue::Discovery => true,
                    TemplateLibraryIssue::NamedSource(source_name) => source_name == name,
                    TemplateLibraryIssue::BuiltinSource => false,
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
        if self.configurations.has_omissions() || !self.issues.is_empty() {
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
                if !candidates.is_empty() || self.has_unresolved_reference(name) =>
            {
                return MissingLibraryLookup::Inconclusive;
            }
            LoadableLibraryLookup::Inconclusive(_) | LoadableLibraryLookup::Absent => {}
        }
        let candidates = self.available_library_candidates(name);
        if !candidates.is_empty() {
            let mut apps: Vec<_> = candidates
                .iter()
                .filter_map(|candidate| candidate.available_app().cloned())
                .collect();
            apps.dedup();
            if !apps.is_empty() {
                return MissingLibraryLookup::FoundInApps(AvailableAppCandidates(apps));
            }
        }
        if self.configurations.has_omissions()
            || self.issues.iter().any(|issue| match issue {
                TemplateLibraryIssue::Discovery => true,
                TemplateLibraryIssue::NamedSource(source_name) => source_name == name,
                TemplateLibraryIssue::BuiltinSource => false,
            })
        {
            MissingLibraryLookup::Inconclusive
        } else {
            MissingLibraryLookup::Absent
        }
    }

    pub fn resolved_libraries(&self) -> impl Iterator<Item = &TemplateLibrary> + '_ {
        let mut indexes = Vec::new();
        for configuration in self.configurations.known() {
            for backend in configuration {
                for index in backend
                    .loadable_by_name
                    .values()
                    .filter_map(|reference| match reference {
                        TemplateLibraryReference::Resolved(index) => Some(*index),
                        TemplateLibraryReference::Unresolved => None,
                    })
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
        let known_configurations = self.configurations.known();
        let closed_single_backend = !self.configurations.has_omissions()
            && known_configurations.len() == 1
            && known_configurations[0].len() == 1;
        self.resolved_libraries()
            .filter(move |_| closed_single_backend)
    }

    fn has_unresolved_reference(&self, name: &LibraryName) -> bool {
        self.configurations.known().iter().flatten().any(|backend| {
            backend.loadable_by_name.get(name) == Some(&TemplateLibraryReference::Unresolved)
        })
    }

    fn builtin_libraries(&self) -> impl Iterator<Item = &TemplateLibrary> + '_ {
        self.resolved_libraries()
            .filter(|library| matches!(&library.kind, TemplateLibraryKind::Builtin))
    }

    fn installed_libraries(&self) -> impl Iterator<Item = (&LibraryName, &TemplateLibrary)> + '_ {
        self.resolved_libraries().filter_map(|library| {
            let TemplateLibraryKind::Installed { load_name } = &library.kind else {
                return None;
            };
            Some((load_name, library))
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
        match &library.kind {
            TemplateLibraryKind::Builtin => {
                let index = self.libraries.len();
                self.libraries.push(library);
                index
            }
            TemplateLibraryKind::Installed { load_name } => {
                let load_name = load_name.clone();
                let index = self.libraries.len();
                self.libraries.push(library);
                self.installed_by_name.insert(load_name, index);
                index
            }
            TemplateLibraryKind::Available { load_name, app } => {
                let load_name = load_name.clone();
                let app = app.clone();
                let module_name = library.module_name().clone();
                if let Some(existing_index) = self.libraries.iter().position(|existing| {
                    matches!(
                        &existing.kind,
                        TemplateLibraryKind::Available {
                            load_name: existing_name,
                            app: existing_app,
                        } if existing_name == &load_name && existing_app == &app
                    ) && existing.module_name() == &module_name
                }) {
                    let indexes = self.available_by_name.entry(load_name).or_default();
                    if !indexes.contains(&existing_index) {
                        indexes.push(existing_index);
                    }
                    return existing_index;
                }

                let index = self.libraries.len();
                self.libraries.push(library);
                self.available_by_name
                    .entry(load_name)
                    .or_default()
                    .push(index);
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
                        .push(TemplateLibraryIssue::NamedSource(candidate.name.clone()));
                }
                TemplateLibraryAnalysis::ParsedNotLibrary { source } => {
                    if source == TemplateLibrarySource::Recovered {
                        self.issues
                            .push(TemplateLibraryIssue::NamedSource(candidate.name.clone()));
                    }
                }
                TemplateLibraryAnalysis::Library { symbols, source } => {
                    if source == TemplateLibrarySource::Recovered {
                        self.issues
                            .push(TemplateLibraryIssue::NamedSource(candidate.name.clone()));
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
            loadable_by_name: resolved_library_references(&app_libraries),
            builtin_indices: Vec::new(),
        });
    }
    libraries
        .configurations
        .replace_known(vec![backend_configurations]);
    libraries.insert_available_candidates(db, project, &installed_template_library_modules);
    libraries
}

fn resolved_library_references(
    libraries: &BTreeMap<LibraryName, usize>,
) -> BTreeMap<LibraryName, TemplateLibraryReference> {
    libraries
        .iter()
        .map(|(name, index)| (name.clone(), TemplateLibraryReference::Resolved(*index)))
        .collect()
}

fn insert_backend_libraries(
    db: &dyn ProjectDb,
    project: Project,
    backend: &TemplateBackend,
    app_libraries: &BTreeMap<LibraryName, usize>,
    libraries: &mut TemplateLibraries,
) -> TemplateBackendLibraries {
    if !backend.is_fully_extracted() {
        libraries
            .configurations
            .omit(TemplateConfigurationOmission::Backend);
    }

    let mut result = TemplateBackendLibraries {
        loadable_by_name: resolved_library_references(app_libraries),
        builtin_indices: Vec::new(),
    };
    for (load_name, module_name) in &backend.libraries {
        let Ok(load_name) = LibraryName::parse(load_name) else {
            libraries
                .configurations
                .omit(TemplateConfigurationOmission::Backend);
            continue;
        };
        let Some((module, symbols, source)) =
            library_from_module_name(db, project, module_name.clone())
        else {
            result
                .loadable_by_name
                .insert(load_name, TemplateLibraryReference::Unresolved);
            continue;
        };
        if source == TemplateLibrarySource::Recovered {
            libraries
                .issues
                .push(TemplateLibraryIssue::NamedSource(load_name.clone()));
        }
        let index = libraries.insert_library(TemplateLibrary::installed(
            load_name.clone(),
            module,
            symbols,
        ));
        result
            .loadable_by_name
            .insert(load_name, TemplateLibraryReference::Resolved(index));
    }

    let builtins = DEFAULT_TEMPLATE_BUILTINS
        .iter()
        .map(|name| PythonModuleName::parse(name).expect("default builtin is a valid module name"))
        .chain(backend.builtins.iter().cloned());
    for module_name in builtins {
        match library_from_module_name(db, project, module_name) {
            Some((module, symbols, source)) => {
                if source == TemplateLibrarySource::Recovered {
                    libraries.issues.push(TemplateLibraryIssue::BuiltinSource);
                }
                let index = libraries.insert_library(TemplateLibrary::builtin(module, symbols));
                result.builtin_indices.push(index);
            }
            None => libraries.issues.push(TemplateLibraryIssue::BuiltinSource),
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
        templatetag_candidates_in_package(db, project, package_module).into_parts();
    let mut issues = candidate_issues
        .into_iter()
        .map(|_| TemplateLibraryIssue::Discovery)
        .collect::<Vec<_>>();
    let mut libraries = Vec::new();

    for candidate in candidates {
        match TemplateLibraryAnalysis::from_file(db, candidate.module.file()) {
            TemplateLibraryAnalysis::Failed => {
                issues.push(TemplateLibraryIssue::NamedSource(candidate.name.clone()));
            }
            TemplateLibraryAnalysis::ParsedNotLibrary { source } => {
                if source == TemplateLibrarySource::Recovered {
                    issues.push(TemplateLibraryIssue::NamedSource(candidate.name.clone()));
                }
            }
            TemplateLibraryAnalysis::Library { symbols, source } => {
                if source == TemplateLibrarySource::Recovered {
                    issues.push(TemplateLibraryIssue::NamedSource(candidate.name.clone()));
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

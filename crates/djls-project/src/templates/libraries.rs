use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::slice::Iter;
use std::sync::LazyLock;

use djls_source::File;

use super::candidates::templatetag_candidates;
use super::candidates::templatetag_candidates_in_package;
use super::installed_app_package_module;
use super::names::LibraryName;
use super::names::TemplateSymbolName;
use super::registrations::template_library_definition_facts;
use super::resolution::TemplateBackendScope;
use super::resolution::TemplateBackendScopeKind;
use super::resolution::TemplateBackendSelection;
use super::settings_cases::TemplateBackendCase;
use super::settings_cases::TemplateBackendId;
use super::settings_cases::TemplateBackendSlot;
use super::settings_cases::TemplateEvidenceCompleteness;
use super::settings_cases::TemplateSettingsCaseId;
use super::settings_cases::TemplateSettingsCases;
use super::settings_cases::template_settings_cases;
use super::symbols::SymbolDefinition;
use super::symbols::TemplateSymbol;
use super::symbols::TemplateSymbolKind;
use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::python::PythonModuleName;
use crate::python::PythonSourceModule;
use crate::settings::settings_module_file;
use crate::settings::types::InstalledAppEvidence;

const DEFAULT_TEMPLATE_BUILTINS: &[&str] = &[
    "django.template.defaulttags",
    "django.template.defaultfilters",
    "django.template.loader_tags",
];

#[derive(Clone, Debug, PartialEq, Eq)]
enum TemplateLibraryKind {
    Builtin,
    Loadable {
        load_name: LibraryName,
    },
    AvailableInApp {
        load_name: LibraryName,
        app: PythonModuleName,
    },
}

/// Stable interned identity for one Template Library module.
///
/// Configured-only libraries have no source file. Keeping that absence in the identity prevents
/// settings-case evidence from masquerading as a navigable Python source.
#[salsa::interned(no_lifetime, debug)]
pub struct TemplateLibraryId {
    #[returns(copy)]
    pub file: Option<File>,
    #[returns(ref)]
    pub module: PythonModuleName,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TemplateLibraryModule {
    Source(PythonSourceModule),
    Configured(PythonModuleName),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TemplateSymbolObservation {
    Observed,
    Unobserved,
}

impl TemplateLibraryModule {
    fn name(&self) -> &PythonModuleName {
        match self {
            Self::Source(module) => module.name(),
            Self::Configured(module) => module,
        }
    }

    fn file(&self) -> Option<File> {
        match self {
            Self::Source(module) => Some(module.file()),
            Self::Configured(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateLibrary {
    id: TemplateLibraryId,
    module: TemplateLibraryModule,
    kind: TemplateLibraryKind,
    symbol_observation: TemplateSymbolObservation,
    symbols: Vec<TemplateSymbol>,
    tag_symbols: BTreeMap<String, usize>,
    filter_symbols: BTreeMap<String, usize>,
}

impl TemplateLibrary {
    fn new(
        id: TemplateLibraryId,
        module: TemplateLibraryModule,
        kind: TemplateLibraryKind,
        symbol_observation: TemplateSymbolObservation,
        symbols: Vec<TemplateSymbol>,
    ) -> Self {
        let symbols = merge_symbols(symbols);
        let mut tag_symbols = BTreeMap::new();
        let mut filter_symbols = BTreeMap::new();
        for (index, symbol) in symbols.iter().enumerate() {
            match symbol.kind {
                TemplateSymbolKind::Tag => {
                    tag_symbols.insert(symbol.name().to_string(), index);
                }
                TemplateSymbolKind::Filter => {
                    filter_symbols.insert(symbol.name().to_string(), index);
                }
            }
        }
        Self {
            id,
            module,
            kind,
            symbol_observation,
            symbols,
            tag_symbols,
            filter_symbols,
        }
    }

    #[must_use]
    fn builtin(
        id: TemplateLibraryId,
        module: PythonSourceModule,
        symbols: Vec<TemplateSymbol>,
    ) -> Self {
        Self::new(
            id,
            TemplateLibraryModule::Source(module),
            TemplateLibraryKind::Builtin,
            TemplateSymbolObservation::Observed,
            symbols,
        )
    }

    #[must_use]
    pub(crate) fn configured_builtin(
        id: TemplateLibraryId,
        module: PythonModuleName,
        symbols: Vec<TemplateSymbol>,
    ) -> Self {
        Self::new(
            id,
            TemplateLibraryModule::Configured(module),
            TemplateLibraryKind::Builtin,
            TemplateSymbolObservation::Observed,
            symbols,
        )
    }

    #[must_use]
    fn source_less_builtin(id: TemplateLibraryId, module: PythonModuleName) -> Self {
        Self::new(
            id,
            TemplateLibraryModule::Configured(module),
            TemplateLibraryKind::Builtin,
            TemplateSymbolObservation::Unobserved,
            Vec::new(),
        )
    }

    #[must_use]
    fn loadable(
        id: TemplateLibraryId,
        load_name: LibraryName,
        module: PythonSourceModule,
        symbols: Vec<TemplateSymbol>,
    ) -> Self {
        Self::new(
            id,
            TemplateLibraryModule::Source(module),
            TemplateLibraryKind::Loadable { load_name },
            TemplateSymbolObservation::Observed,
            symbols,
        )
    }

    #[must_use]
    pub(crate) fn configured_loadable(
        id: TemplateLibraryId,
        load_name: LibraryName,
        module: PythonModuleName,
        symbols: Vec<TemplateSymbol>,
    ) -> Self {
        Self::new(
            id,
            TemplateLibraryModule::Configured(module),
            TemplateLibraryKind::Loadable { load_name },
            TemplateSymbolObservation::Observed,
            symbols,
        )
    }

    #[must_use]
    fn source_less_loadable(
        id: TemplateLibraryId,
        load_name: LibraryName,
        module: PythonModuleName,
    ) -> Self {
        Self::new(
            id,
            TemplateLibraryModule::Configured(module),
            TemplateLibraryKind::Loadable { load_name },
            TemplateSymbolObservation::Unobserved,
            Vec::new(),
        )
    }

    #[must_use]
    pub(crate) fn configured_available_in_app(
        id: TemplateLibraryId,
        load_name: LibraryName,
        app: PythonModuleName,
        module: PythonModuleName,
        symbols: Vec<TemplateSymbol>,
    ) -> Self {
        Self::new(
            id,
            TemplateLibraryModule::Configured(module),
            TemplateLibraryKind::AvailableInApp { load_name, app },
            TemplateSymbolObservation::Observed,
            symbols,
        )
    }

    #[must_use]
    fn available_in_app(
        id: TemplateLibraryId,
        load_name: LibraryName,
        app: PythonModuleName,
        module: PythonSourceModule,
        symbols: Vec<TemplateSymbol>,
    ) -> Self {
        Self::new(
            id,
            TemplateLibraryModule::Source(module),
            TemplateLibraryKind::AvailableInApp { load_name, app },
            TemplateSymbolObservation::Observed,
            symbols,
        )
    }

    #[must_use]
    pub fn load_name(&self) -> Option<&LibraryName> {
        match &self.kind {
            TemplateLibraryKind::Builtin => None,
            TemplateLibraryKind::Loadable { load_name }
            | TemplateLibraryKind::AvailableInApp { load_name, .. } => Some(load_name),
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

    /// Return the resolved Python source when one was observed.
    #[must_use]
    pub fn source_file(&self) -> Option<File> {
        self.module.file()
    }

    #[must_use]
    pub fn id(&self) -> TemplateLibraryId {
        self.id
    }

    #[must_use]
    pub fn symbols(&self) -> &[TemplateSymbol] {
        &self.symbols
    }

    #[must_use]
    pub fn symbol(&self, kind: TemplateSymbolKind, name: &str) -> Option<&TemplateSymbol> {
        let index = match kind {
            TemplateSymbolKind::Tag => self.tag_symbols.get(name),
            TemplateSymbolKind::Filter => self.filter_symbols.get(name),
        }?;
        self.symbols.get(*index)
    }

    /// Whether source discovery left this library's symbol names unobserved.
    #[must_use]
    pub fn symbols_are_unobserved(&self) -> bool {
        matches!(
            self.symbol_observation,
            TemplateSymbolObservation::Unobserved
        )
    }

    fn insert_configured_tag(&mut self, name: &str) -> bool {
        if self.symbol(TemplateSymbolKind::Tag, name).is_some() {
            return false;
        }
        let Ok(name) = TemplateSymbolName::parse(name) else {
            return false;
        };
        self.symbols.push(TemplateSymbol {
            kind: TemplateSymbolKind::Tag,
            name,
            definition: SymbolDefinition::Unknown,
            doc: None,
        });
        self.symbols
            .sort_by(|left, right| left.kind.cmp(&right.kind).then(left.name.cmp(&right.name)));
        self.tag_symbols.clear();
        self.filter_symbols.clear();
        for (index, symbol) in self.symbols.iter().enumerate() {
            match symbol.kind {
                TemplateSymbolKind::Tag => {
                    self.tag_symbols.insert(symbol.name().to_string(), index);
                }
                TemplateSymbolKind::Filter => {
                    self.filter_symbols.insert(symbol.name().to_string(), index);
                }
            }
        }
        true
    }

    #[must_use]
    fn available_in_app_module(&self) -> Option<&PythonModuleName> {
        match &self.kind {
            TemplateLibraryKind::AvailableInApp { app, .. } => Some(app),
            TemplateLibraryKind::Builtin | TemplateLibraryKind::Loadable { .. } => None,
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
            let existing_doc = existing
                .doc
                .as_deref()
                .filter(|doc| !doc.trim().is_empty())
                .map(str::trim);
            let new_doc = new_symbol
                .doc
                .as_deref()
                .filter(|doc| !doc.trim().is_empty())
                .map(str::trim);
            if new_doc > existing_doc {
                existing.doc.clone_from(&new_symbol.doc);
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
    /// Return the library only when every feasible settings case agrees.
    #[must_use]
    pub fn found(self) -> Option<&'a TemplateLibrary> {
        match self {
            Self::Found(library) => Some(library),
            Self::Ambiguous(_) | Self::Inconclusive(_) | Self::Absent => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppTemplateSymbolLookup {
    FoundInApp {
        app: PythonModuleName,
        load_name: LibraryName,
    },
    Absent,
    Inconclusive,
}

/// Consumer-shaped certainty for a tag or filter in the selected Template Backend Scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScopedTemplateSymbolLookup {
    Builtin,
    RequiresLoad(Vec<LibraryName>),
    Inconclusive,
    Absent,
}

/// The library supplying a symbol in one feasible template backend.
///
/// `Known(None)` is significant: absence in one backend disagrees with a definition in another.
/// `Unobserved` retains the identity of a definite source-less library whose open symbol inventory
/// may contain the requested name. `Unknown` is reserved for uncertainty with no library identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectiveDefinitionLibrary<'a> {
    Known(Option<&'a TemplateLibrary>),
    Unobserved(&'a TemplateLibrary),
    Unknown,
}

/// One ordered Template Library update in a feasible backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TemplateLibraryChainStep<'a> {
    Library(&'a TemplateLibrary),
    Unknown,
}

/// Builtins followed by loaded libraries for one feasible backend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateLibraryChain<'a>(Vec<TemplateLibraryChainStep<'a>>);

impl<'a> TemplateLibraryChain<'a> {
    #[must_use]
    pub fn steps(&self) -> &[TemplateLibraryChainStep<'a>] {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateLibraryAppCandidates(Vec<PythonModuleName>);

impl TemplateLibraryAppCandidates {
    /// Return the first app candidate in deterministic lookup order.
    ///
    /// # Panics
    ///
    /// Panics only if the private non-empty candidate invariant is violated.
    #[must_use]
    pub fn primary(&self) -> &PythonModuleName {
        self.0
            .first()
            .expect("available-in-app candidates should be non-empty")
    }

    #[must_use]
    pub fn as_slice(&self) -> &[PythonModuleName] {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MissingTemplateLibraryLookup {
    FoundInApps(TemplateLibraryAppCandidates),
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
enum TemplateSettingsOmission {
    Settings,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TemplateLibrarySlot {
    Backend(TemplateBackendId, TemplateBackendLibraries),
    Remainder,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TemplateLibrarySettingsCase {
    id: TemplateSettingsCaseId,
    slots: Vec<TemplateLibrarySlot>,
    guidance: TemplateEvidenceCompleteness,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TemplateLibrarySettingsCases {
    Standalone {
        backend: TemplateBackendLibraries,
        omissions: Vec<TemplateSettingsOmission>,
    },
    Exhaustive(Vec<TemplateLibrarySettingsCase>),
    WithOmissions {
        known: Vec<TemplateLibrarySettingsCase>,
        omissions: Vec<TemplateSettingsOmission>,
    },
}

impl TemplateLibrarySettingsCases {
    fn known(&self) -> &[TemplateLibrarySettingsCase] {
        match self {
            Self::Standalone { .. } => &[],
            Self::Exhaustive(known) | Self::WithOmissions { known, .. } => known,
        }
    }

    fn standalone(&self) -> Option<&TemplateBackendLibraries> {
        match self {
            Self::Standalone { backend, .. } => Some(backend),
            Self::Exhaustive(_) | Self::WithOmissions { .. } => None,
        }
    }

    const fn has_omissions(&self) -> bool {
        match self {
            Self::Standalone { omissions, .. } | Self::WithOmissions { omissions, .. } => {
                !omissions.is_empty()
            }
            Self::Exhaustive(_) => false,
        }
    }

    fn has_unknown_loadables(&self) -> bool {
        self.standalone().is_some_and(|backend| {
            backend.loadables_completeness.is_open()
                || backend.apps_completeness.is_open()
                || backend.discovery_completeness.is_open()
        }) || self.known().iter().any(|settings_case| {
            settings_case.slots.iter().any(|slot| match slot {
                TemplateLibrarySlot::Backend(_, backend) => {
                    backend.loadables_completeness.is_open()
                        || backend.apps_completeness.is_open()
                        || backend.discovery_completeness.is_open()
                }
                TemplateLibrarySlot::Remainder => true,
            })
        })
    }

    fn replace_known(&mut self, known: Vec<TemplateLibrarySettingsCase>) {
        *self = if self.has_omissions() {
            Self::WithOmissions {
                known,
                omissions: vec![TemplateSettingsOmission::Settings],
            }
        } else {
            Self::Exhaustive(known)
        };
    }

    fn backend(
        &self,
        id: TemplateBackendId,
    ) -> Option<(&TemplateLibrarySettingsCase, &TemplateBackendLibraries)> {
        self.known().iter().find_map(|settings_case| {
            settings_case.slots.iter().find_map(|slot| match slot {
                TemplateLibrarySlot::Backend(candidate, backend) if *candidate == id => {
                    Some((settings_case, backend))
                }
                TemplateLibrarySlot::Backend(_, _) | TemplateLibrarySlot::Remainder => None,
            })
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TemplateLibraryIndexEntry {
    Resolved(usize),
    Unresolved { known_candidate: Option<usize> },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TemplateBackendLibraries {
    loadable_by_name: BTreeMap<LibraryName, TemplateLibraryIndexEntry>,
    builtin_indices: Vec<usize>,
    /// Uncertainty about this ID-bearing backend's type or backend-specific behavior.
    backend_completeness: TemplateEvidenceCompleteness,
    /// Uncertainty is owned by the backend field that produced it. This keeps an unrelated
    /// backend or sibling field from weakening an otherwise exact lookup after file narrowing.
    loadables_completeness: TemplateEvidenceCompleteness,
    builtins_completeness: TemplateEvidenceCompleteness,
    apps_completeness: TemplateEvidenceCompleteness,
    discovery_completeness: TemplateEvidenceCompleteness,
    app_names_after_remainder: BTreeSet<LibraryName>,
    authoritative_names: BTreeSet<LibraryName>,
}

impl TemplateBackendLibraries {
    fn load_name_is_open(&self, name: &LibraryName) -> bool {
        self.load_name_is_open_str(name.as_str())
    }

    fn load_name_is_open_str(&self, name: &str) -> bool {
        if self.authoritative_names.contains(name) {
            return false;
        }

        self.loadables_completeness.is_open()
            || ((self.apps_completeness.is_open() || self.discovery_completeness.is_open())
                && !self.app_names_after_remainder.contains(name))
    }
}

type TestingBackendSettings = (Vec<(LibraryName, PythonModuleName)>, Vec<PythonModuleName>);
type DiscoveredLibrary = (
    LibraryName,
    TemplateLibraryId,
    PythonSourceModule,
    Vec<TemplateSymbol>,
);

struct InstalledAppLibraries {
    evidence: Vec<InstalledAppEvidence>,
    libraries: BTreeMap<LibraryName, usize>,
    app_remainder: bool,
    discovery_remainder: bool,
    names_after_remainder: BTreeSet<LibraryName>,
    unresolved_names: BTreeMap<LibraryName, Option<usize>>,
}

enum ConfiguredLibraryModule {
    Source {
        id: TemplateLibraryId,
        module: PythonSourceModule,
        symbols: Vec<TemplateSymbol>,
        recovered: bool,
    },
    SourceLess {
        id: TemplateLibraryId,
        module: PythonModuleName,
    },
    NotLibrary,
}

/// Project-wide Template Library catalog.
///
/// The catalog indexes definitions and load names while retaining backend-correlated availability,
/// open evidence, and omission causes. File-specific access belongs to `ScopedTemplateLibraries`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateLibraryCatalog {
    libraries: Vec<TemplateLibrary>,
    definitions_by_name: BTreeMap<TemplateSymbolKind, BTreeMap<String, Vec<usize>>>,
    loadable_by_name: BTreeMap<LibraryName, usize>,
    settings_cases: TemplateLibrarySettingsCases,
    available_in_app_by_name: BTreeMap<LibraryName, Vec<usize>>,
    issues: Vec<TemplateLibraryIssue>,
}

impl Default for TemplateLibraryCatalog {
    fn default() -> Self {
        Self {
            libraries: Vec::new(),
            definitions_by_name: BTreeMap::new(),
            loadable_by_name: BTreeMap::new(),
            settings_cases: TemplateLibrarySettingsCases::Standalone {
                backend: TemplateBackendLibraries::default(),
                omissions: vec![TemplateSettingsOmission::Settings],
            },
            available_in_app_by_name: BTreeMap::new(),
            issues: Vec::new(),
        }
    }
}

#[derive(Clone, Copy)]
enum LibraryBackendAlternative<'a> {
    Known {
        backend: &'a TemplateBackendLibraries,
        guidance: TemplateEvidenceCompleteness,
    },
    NoBackend {
        guidance: TemplateEvidenceCompleteness,
    },
    Unknown,
}

impl<'a> LibraryBackendAlternative<'a> {
    fn backend(self) -> Option<&'a TemplateBackendLibraries> {
        match self {
            Self::Known { backend, .. } => Some(backend),
            Self::NoBackend { .. } | Self::Unknown => None,
        }
    }

    fn guidance_is_open(self) -> bool {
        match self {
            Self::Known { guidance, .. } | Self::NoBackend { guidance } => guidance.is_open(),
            Self::Unknown => true,
        }
    }

    fn backend_is_open(self) -> bool {
        match self {
            Self::Known { backend, .. } => backend.backend_completeness.is_open(),
            Self::NoBackend { .. } => false,
            Self::Unknown => true,
        }
    }

    fn builtins_are_open(self) -> bool {
        match self {
            Self::Known { backend, .. } => backend.builtins_completeness.is_open(),
            Self::NoBackend { .. } => false,
            Self::Unknown => true,
        }
    }

    fn load_name_is_open(self, name: &LibraryName) -> bool {
        match self {
            Self::Known { backend, .. } => backend.load_name_is_open(name),
            Self::NoBackend { .. } => false,
            Self::Unknown => true,
        }
    }

    fn loadable(self, name: &LibraryName) -> Option<TemplateLibraryIndexEntry> {
        self.backend()?.loadable_by_name.get(name).copied()
    }

    fn has_open_inventory(self) -> bool {
        self.guidance_is_open()
            || self.backend_is_open()
            || self.backend().is_some_and(|backend| {
                backend.loadables_completeness.is_open()
                    || backend.apps_completeness.is_open()
                    || backend.discovery_completeness.is_open()
            })
    }
}

#[derive(Clone, Copy)]
struct LibraryScopeView<'a> {
    libraries: &'a TemplateLibraryCatalog,
    scope: &'a TemplateBackendScope,
}

impl<'a> LibraryScopeView<'a> {
    const fn new(libraries: &'a TemplateLibraryCatalog, scope: &'a TemplateBackendScope) -> Self {
        Self { libraries, scope }
    }

    fn project_inventory(libraries: &'a TemplateLibraryCatalog) -> Self {
        Self::new(libraries, TemplateBackendScope::project_inventory_ref())
    }

    fn alternatives(self) -> LibraryAlternativeIter<'a> {
        let kind = match self.scope.kind() {
            TemplateBackendScopeKind::ProjectInventory => {
                LibraryAlternativeIterKind::ProjectInventory {
                    standalone: self.libraries.settings_cases.standalone(),
                    settings_cases: self.libraries.settings_cases.known().iter(),
                    current: None,
                }
            }
            TemplateBackendScopeKind::Selected(selections) => {
                LibraryAlternativeIterKind::Scoped(selections.as_slice().iter())
            }
        };
        LibraryAlternativeIter {
            libraries: self.libraries,
            kind,
        }
    }

    fn has_omissions(self) -> bool {
        matches!(
            self.scope.kind(),
            TemplateBackendScopeKind::ProjectInventory
        ) && self.libraries.settings_cases.has_omissions()
    }

    fn is_selected(self) -> bool {
        matches!(self.scope.kind(), TemplateBackendScopeKind::Selected(_))
    }
}

struct CurrentLibraryCase<'a> {
    settings_case: &'a TemplateLibrarySettingsCase,
    slots: Iter<'a, TemplateLibrarySlot>,
}

enum LibraryAlternativeIterKind<'a> {
    ProjectInventory {
        standalone: Option<&'a TemplateBackendLibraries>,
        settings_cases: Iter<'a, TemplateLibrarySettingsCase>,
        current: Option<CurrentLibraryCase<'a>>,
    },
    Scoped(Iter<'a, TemplateBackendSelection>),
}

struct LibraryAlternativeIter<'a> {
    libraries: &'a TemplateLibraryCatalog,
    kind: LibraryAlternativeIterKind<'a>,
}

impl<'a> Iterator for LibraryAlternativeIter<'a> {
    type Item = LibraryBackendAlternative<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.kind {
            LibraryAlternativeIterKind::ProjectInventory {
                standalone,
                settings_cases,
                current,
            } => loop {
                if let Some(backend) = standalone.take() {
                    return Some(LibraryBackendAlternative::Known {
                        backend,
                        guidance: TemplateEvidenceCompleteness::Complete,
                    });
                }
                if let Some(state) = current {
                    if let Some(slot) = state.slots.next() {
                        return Some(match slot {
                            TemplateLibrarySlot::Backend(_, backend) => {
                                LibraryBackendAlternative::Known {
                                    backend,
                                    guidance: state.settings_case.guidance,
                                }
                            }
                            TemplateLibrarySlot::Remainder => LibraryBackendAlternative::Unknown,
                        });
                    }
                    *current = None;
                }

                let settings_case = settings_cases.next()?;
                if settings_case.slots.is_empty() {
                    return Some(LibraryBackendAlternative::NoBackend {
                        guidance: settings_case.guidance,
                    });
                }
                *current = Some(CurrentLibraryCase {
                    settings_case,
                    slots: settings_case.slots.iter(),
                });
            },
            LibraryAlternativeIterKind::Scoped(selections) => {
                let selection = selections.next()?;
                Some(match *selection {
                    TemplateBackendSelection::Backend(backend) => {
                        self.libraries.settings_cases.backend(backend).map_or(
                            LibraryBackendAlternative::Unknown,
                            |(settings_case, backend)| LibraryBackendAlternative::Known {
                                backend,
                                guidance: settings_case.guidance,
                            },
                        )
                    }
                    TemplateBackendSelection::SettingsCaseRemainder(_) => {
                        LibraryBackendAlternative::Unknown
                    }
                })
            }
        }
    }
}

impl TemplateLibraryCatalog {
    pub(crate) fn loadable_library_in_scope<'a>(
        &'a self,
        scope: &'a TemplateBackendScope,
        name: &LibraryName,
    ) -> LoadableLibraryLookup<'a> {
        self.loadable_library_in_view(LibraryScopeView::new(self, scope), name)
    }

    fn loadable_library_in_view<'a>(
        &'a self,
        view: LibraryScopeView<'a>,
        name: &LibraryName,
    ) -> LoadableLibraryLookup<'a> {
        let mut outcomes = Vec::new();
        let mut indexes = Vec::new();
        let mut unresolved = false;
        for backend in view.alternatives() {
            let mut matches = Vec::new();
            let mut absent = false;
            if (view.is_selected() && backend.backend_is_open()) || backend.load_name_is_open(name)
            {
                unresolved = true;
            }
            if backend.backend_is_open() && backend.loadable(name).is_none() {
                outcomes.push((matches, absent));
                continue;
            }
            match backend.loadable(name) {
                Some(TemplateLibraryIndexEntry::Resolved(index)) => {
                    matches.push(index);
                    if !indexes.contains(&index) {
                        indexes.push(index);
                    }
                }
                Some(TemplateLibraryIndexEntry::Unresolved { known_candidate }) => {
                    unresolved = true;
                    if let Some(index) = known_candidate {
                        matches.push(index);
                        if !indexes.contains(&index) {
                            indexes.push(index);
                        }
                    }
                }
                None => absent = true,
            }
            outcomes.push((matches, absent));
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

        if view.has_omissions()
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

    pub(crate) fn loadable_library_str_in_scope<'a>(
        &'a self,
        scope: &'a TemplateBackendScope,
        name: &str,
    ) -> LoadableLibraryLookup<'a> {
        match LibraryName::parse(name) {
            Ok(name) => self.loadable_library_in_scope(scope, &name),
            Err(_) => LoadableLibraryLookup::Absent,
        }
    }

    pub(crate) fn completion_library_names_in_scope(
        &self,
        scope: &TemplateBackendScope,
    ) -> Vec<LibraryName> {
        Self::completion_library_names_in_view(LibraryScopeView::new(self, scope))
    }

    fn completion_library_names_in_view(view: LibraryScopeView<'_>) -> Vec<LibraryName> {
        view.alternatives()
            .filter_map(LibraryBackendAlternative::backend)
            .flat_map(|backend| backend.loadable_by_name.keys())
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub(crate) fn resolved_libraries_in_scope(
        &self,
        scope: &TemplateBackendScope,
    ) -> Vec<&TemplateLibrary> {
        self.resolved_libraries_in_view(LibraryScopeView::new(self, scope))
    }

    fn resolved_libraries_in_view(&self, view: LibraryScopeView<'_>) -> Vec<&TemplateLibrary> {
        let mut indexes = Vec::new();
        for backend in view
            .alternatives()
            .filter_map(LibraryBackendAlternative::backend)
        {
            for index in backend
                .loadable_by_name
                .values()
                .filter_map(|reference| match reference {
                    TemplateLibraryIndexEntry::Resolved(index) => Some(*index),
                    TemplateLibraryIndexEntry::Unresolved { known_candidate } => *known_candidate,
                })
                .chain(backend.builtin_indices.iter().copied())
            {
                if !indexes.contains(&index) {
                    indexes.push(index);
                }
            }
        }
        indexes
            .into_iter()
            .filter_map(|index| self.libraries.get(index))
            .collect()
    }

    pub(crate) fn scoped_symbol_candidates_in_scope(
        &self,
        scope: &TemplateBackendScope,
        name: &str,
        kind: TemplateSymbolKind,
    ) -> Vec<TemplateSymbolCandidate> {
        self.scoped_symbol_candidates_in_view(LibraryScopeView::new(self, scope), name, kind)
    }

    fn scoped_symbol_candidates_in_view(
        &self,
        view: LibraryScopeView<'_>,
        name: &str,
        kind: TemplateSymbolKind,
    ) -> Vec<TemplateSymbolCandidate> {
        let lookup = self.scoped_symbol_lookup_in_view(view, name, kind);
        let libraries = self.resolved_libraries_in_view(view);
        let mut candidates = Vec::new();

        if matches!(lookup, ScopedTemplateSymbolLookup::Builtin) {
            let candidate = libraries
                .iter()
                .filter(|library| matches!(&library.kind, TemplateLibraryKind::Builtin))
                .filter_map(|library| {
                    library
                        .symbol(kind, name)
                        .map(|symbol| TemplateSymbolCandidate {
                            symbol: symbol.clone(),
                            availability: TemplateSymbolAvailability::Builtin {
                                module: library.module_name().clone(),
                            },
                        })
                })
                .next_back();
            candidates.extend(candidate);
        }

        let ScopedTemplateSymbolLookup::RequiresLoad(required_names) = lookup else {
            return candidates;
        };
        for library in libraries
            .into_iter()
            .filter(|library| matches!(&library.kind, TemplateLibraryKind::Loadable { .. }))
        {
            let Some(load_name) = library
                .load_name()
                .filter(|name| required_names.contains(name))
            else {
                continue;
            };
            if let Some(symbol) = library.symbol(kind, name) {
                candidates.push(TemplateSymbolCandidate {
                    symbol: symbol.clone(),
                    availability: TemplateSymbolAvailability::RequiresLoad {
                        load_name: load_name.clone(),
                    },
                });
            }
        }
        candidates
    }

    pub(crate) fn scoped_symbol_lookup_in_scope(
        &self,
        scope: &TemplateBackendScope,
        name: &str,
        kind: TemplateSymbolKind,
    ) -> ScopedTemplateSymbolLookup {
        self.scoped_symbol_lookup_in_view(LibraryScopeView::new(self, scope), name, kind)
    }

    fn scoped_symbol_lookup_in_view(
        &self,
        view: LibraryScopeView<'_>,
        name: &str,
        kind: TemplateSymbolKind,
    ) -> ScopedTemplateSymbolLookup {
        let mut builtin_present = false;
        let mut builtin_absent = false;
        let mut inconclusive = false;

        for backend in view.alternatives() {
            if backend.builtins_are_open() {
                inconclusive = true;
            }
            let mut present = false;
            let mut open = false;
            if let Some(backend) = backend.backend() {
                for library in backend
                    .builtin_indices
                    .iter()
                    .filter_map(|index| self.libraries.get(*index))
                {
                    present |= library.symbol(kind, name).is_some();
                    open |= library.symbols_are_unobserved();
                }
            }
            builtin_present |= present;
            builtin_absent |= !present && !open;
            inconclusive |= !present && open;
        }
        if builtin_present && !builtin_absent && !inconclusive {
            return ScopedTemplateSymbolLookup::Builtin;
        }
        if builtin_present {
            inconclusive = true;
        }

        let mut required = Vec::new();
        for load_name in Self::completion_library_names_in_view(view) {
            let mut present = false;
            let mut absent = false;
            let mut open = false;
            for backend in view.alternatives() {
                open |= backend.load_name_is_open(&load_name);
                match backend.loadable(&load_name) {
                    Some(TemplateLibraryIndexEntry::Resolved(index)) => {
                        if let Some(library) = self.libraries.get(index) {
                            let has_symbol = library.symbol(kind, name).is_some();
                            present |= has_symbol;
                            open |= !has_symbol && library.symbols_are_unobserved();
                            absent |= !has_symbol && !library.symbols_are_unobserved();
                        }
                    }
                    Some(TemplateLibraryIndexEntry::Unresolved { .. }) => open = true,
                    None => absent = true,
                }
            }
            if present && !absent && !open {
                required.push(load_name);
            } else if present || open {
                inconclusive = true;
            }
        }

        if inconclusive {
            ScopedTemplateSymbolLookup::Inconclusive
        } else if !required.is_empty() {
            ScopedTemplateSymbolLookup::RequiresLoad(required)
        } else if view.has_omissions() {
            ScopedTemplateSymbolLookup::Inconclusive
        } else {
            ScopedTemplateSymbolLookup::Absent
        }
    }

    pub(crate) fn template_symbol_lookup_in_scope(
        &self,
        scope: &TemplateBackendScope,
        name: &str,
        kind: TemplateSymbolKind,
    ) -> AppTemplateSymbolLookup {
        self.template_symbol_lookup_in_view(LibraryScopeView::new(self, scope), name, kind)
    }

    fn template_symbol_lookup_in_view(
        &self,
        view: LibraryScopeView<'_>,
        name: &str,
        kind: TemplateSymbolKind,
    ) -> AppTemplateSymbolLookup {
        let candidates = self.available_symbol_candidates(name, kind);
        let has_candidates = !candidates.is_empty();
        let mut has_uncertain_candidate = false;
        for candidate in candidates {
            let (Some(app), Some(load_name)) =
                (candidate.available_in_app_module(), candidate.load_name())
            else {
                continue;
            };
            let mut shadowed = false;
            let mut unshadowed = false;
            let mut open = view
                .alternatives()
                .any(LibraryBackendAlternative::guidance_is_open);
            for alternative in view.alternatives() {
                let Some(backend) = alternative.backend() else {
                    match alternative {
                        LibraryBackendAlternative::NoBackend { .. } => unshadowed = true,
                        LibraryBackendAlternative::Unknown => open = true,
                        LibraryBackendAlternative::Known { .. } => unreachable!(),
                    }
                    continue;
                };
                if backend.authoritative_names.contains(load_name) {
                    shadowed = true;
                    if let Some(TemplateLibraryIndexEntry::Resolved(index)) =
                        backend.loadable_by_name.get(load_name)
                        && self.libraries.get(*index).is_some_and(|library| {
                            library.symbol(kind, name).is_none() && library.symbols_are_unobserved()
                        })
                    {
                        open = true;
                    }
                } else if backend.load_name_is_open(load_name) {
                    open = true;
                } else {
                    unshadowed = true;
                }
            }
            if !shadowed && !unshadowed && !open {
                unshadowed = true;
            }
            if open || (shadowed && unshadowed) {
                has_uncertain_candidate = true;
                continue;
            }
            if unshadowed {
                return AppTemplateSymbolLookup::FoundInApp {
                    app: app.clone(),
                    load_name: load_name.clone(),
                };
            }
        }
        if has_uncertain_candidate {
            AppTemplateSymbolLookup::Inconclusive
        } else if has_candidates {
            AppTemplateSymbolLookup::Absent
        } else if view.has_omissions()
            || view
                .alternatives()
                .any(LibraryBackendAlternative::has_open_inventory)
            || !self.issues.is_empty()
        {
            AppTemplateSymbolLookup::Inconclusive
        } else {
            AppTemplateSymbolLookup::Absent
        }
    }

    pub(crate) fn missing_library_lookup_in_scope(
        &self,
        scope: &TemplateBackendScope,
        name: &LibraryName,
    ) -> MissingTemplateLibraryLookup {
        self.missing_library_lookup_in_view(LibraryScopeView::new(self, scope), name)
    }

    fn missing_library_lookup_in_view(
        &self,
        view: LibraryScopeView<'_>,
        name: &LibraryName,
    ) -> MissingTemplateLibraryLookup {
        match self.loadable_library_in_view(view, name) {
            LoadableLibraryLookup::Found(_) | LoadableLibraryLookup::Ambiguous(_) => {
                return MissingTemplateLibraryLookup::Inconclusive;
            }
            LoadableLibraryLookup::Inconclusive(candidates)
                if !candidates.is_empty()
                    || view.alternatives().any(|backend| {
                        backend.load_name_is_open(name)
                            || matches!(
                                backend.loadable(name),
                                Some(TemplateLibraryIndexEntry::Unresolved { .. })
                            )
                    }) =>
            {
                return MissingTemplateLibraryLookup::Inconclusive;
            }
            LoadableLibraryLookup::Inconclusive(_) | LoadableLibraryLookup::Absent => {}
        }
        let candidates = self.available_in_app_candidates(name);
        if !candidates.is_empty() {
            if view.alternatives().any(|alternative| {
                alternative.guidance_is_open()
                    || match alternative {
                        LibraryBackendAlternative::Known { backend, .. } => {
                            !backend.authoritative_names.contains(name)
                                && backend.load_name_is_open(name)
                        }
                        LibraryBackendAlternative::NoBackend { .. } => false,
                        LibraryBackendAlternative::Unknown => true,
                    }
            }) {
                return MissingTemplateLibraryLookup::Inconclusive;
            }
            let mut apps: Vec<_> = candidates
                .iter()
                .filter_map(|candidate| candidate.available_in_app_module().cloned())
                .collect();
            apps.dedup();
            if !apps.is_empty() {
                return MissingTemplateLibraryLookup::FoundInApps(TemplateLibraryAppCandidates(
                    apps,
                ));
            }
        }
        if view.has_omissions()
            || view
                .alternatives()
                .any(LibraryBackendAlternative::has_open_inventory)
            || self.issues.iter().any(|issue| match issue {
                TemplateLibraryIssue::Discovery => true,
                TemplateLibraryIssue::NamedSource(source_name) => source_name == name,
                TemplateLibraryIssue::BuiltinSource => false,
            })
        {
            MissingTemplateLibraryLookup::Inconclusive
        } else {
            MissingTemplateLibraryLookup::Absent
        }
    }

    pub(crate) fn effective_definition_libraries_in_scope<'a>(
        &'a self,
        scope: &'a TemplateBackendScope,
        symbol_name: &str,
        kind: TemplateSymbolKind,
        loaded_names: &[&str],
    ) -> Vec<EffectiveDefinitionLibrary<'a>> {
        let mut definitions = Vec::new();
        self.for_each_effective_definition_library_in_scope(
            scope,
            symbol_name,
            kind,
            loaded_names,
            |definition| definitions.push(definition),
        );
        definitions
    }

    pub(crate) fn for_each_effective_definition_library_in_scope<'a>(
        &'a self,
        scope: &'a TemplateBackendScope,
        symbol_name: &str,
        kind: TemplateSymbolKind,
        loaded_names: &[&str],
        visitor: impl FnMut(EffectiveDefinitionLibrary<'a>),
    ) {
        self.for_each_effective_definition_library_in_view(
            LibraryScopeView::new(self, scope),
            symbol_name,
            kind,
            loaded_names,
            visitor,
        );
    }

    fn for_each_effective_definition_library_in_view<'a>(
        &'a self,
        view: LibraryScopeView<'a>,
        symbol_name: &str,
        kind: TemplateSymbolKind,
        loaded_names: &[&str],
        mut visitor: impl FnMut(EffectiveDefinitionLibrary<'a>),
    ) {
        let has_symbol = |library: &TemplateLibrary| library.symbol(kind, symbol_name).is_some();
        let mut visited = false;

        for alternative in view.alternatives() {
            visited = true;
            let Some(backend) = alternative.backend() else {
                visitor(match alternative {
                    LibraryBackendAlternative::NoBackend { .. } => {
                        EffectiveDefinitionLibrary::Known(None)
                    }
                    LibraryBackendAlternative::Unknown => EffectiveDefinitionLibrary::Unknown,
                    LibraryBackendAlternative::Known { .. } => unreachable!(),
                });
                continue;
            };
            let scoped = alternative;
            if scoped.backend_is_open() {
                visitor(EffectiveDefinitionLibrary::Unknown);
                continue;
            }

            let mut effective = None;
            let mut unobserved = None;
            let mut unknown = scoped.builtins_are_open();
            for library in backend
                .builtin_indices
                .iter()
                .filter_map(|index| self.libraries.get(*index))
            {
                if has_symbol(library) {
                    effective = Some(library);
                    unobserved = None;
                } else if library.symbols_are_unobserved() {
                    unobserved = Some(library);
                }
            }

            for loaded_name in loaded_names {
                let loaded_name = loaded_name.trim();
                if loaded_name.is_empty() || loaded_name.chars().any(char::is_whitespace) {
                    continue;
                }
                match backend.loadable_by_name.get(loaded_name) {
                    Some(TemplateLibraryIndexEntry::Resolved(index)) => {
                        if let Some(library) = self.libraries.get(*index) {
                            if has_symbol(library) {
                                effective = Some(library);
                                unobserved = None;
                                unknown = false;
                            } else if library.symbols_are_unobserved() {
                                unobserved = Some(library);
                                unknown = false;
                            }
                        }
                    }
                    Some(TemplateLibraryIndexEntry::Unresolved { .. }) => {
                        unobserved = None;
                        unknown = true;
                    }
                    None if backend.load_name_is_open_str(loaded_name) => {
                        unobserved = None;
                        unknown = true;
                    }
                    None => {}
                }
            }

            visitor(if unknown {
                EffectiveDefinitionLibrary::Unknown
            } else if let Some(library) = unobserved {
                EffectiveDefinitionLibrary::Unobserved(library)
            } else {
                EffectiveDefinitionLibrary::Known(effective)
            });
        }
        if !visited || view.has_omissions() {
            visitor(EffectiveDefinitionLibrary::Unknown);
        }
    }

    pub(crate) fn library_chains_in_scope<'a>(
        &'a self,
        scope: &'a TemplateBackendScope,
        loaded_names: &[&str],
    ) -> Vec<TemplateLibraryChain<'a>> {
        let mut chains = Vec::new();
        self.fold_library_chains_in_scope(scope, loaded_names, Vec::new, Vec::push, |steps| {
            chains.push(TemplateLibraryChain(steps));
        });
        chains
    }

    pub(crate) fn fold_library_chains_in_scope<'a, State>(
        &'a self,
        scope: &'a TemplateBackendScope,
        loaded_names: &[&str],
        initial: impl FnMut() -> State,
        step: impl FnMut(&mut State, TemplateLibraryChainStep<'a>),
        finish: impl FnMut(State),
    ) {
        self.fold_library_chains_in_view(
            LibraryScopeView::new(self, scope),
            loaded_names,
            initial,
            step,
            finish,
        );
    }

    fn fold_library_chains_in_view<'a, State>(
        &'a self,
        view: LibraryScopeView<'a>,
        loaded_names: &[&str],
        mut initial: impl FnMut() -> State,
        mut step: impl FnMut(&mut State, TemplateLibraryChainStep<'a>),
        mut finish: impl FnMut(State),
    ) {
        let mut visited = false;
        for alternative in view.alternatives() {
            visited = true;
            let mut state = initial();
            let Some(backend) = alternative.backend() else {
                match alternative {
                    LibraryBackendAlternative::NoBackend { .. } => {}
                    LibraryBackendAlternative::Unknown => {
                        step(&mut state, TemplateLibraryChainStep::Unknown);
                    }
                    LibraryBackendAlternative::Known { .. } => unreachable!(),
                }
                finish(state);
                continue;
            };
            let scoped = alternative;
            if scoped.backend_is_open() || scoped.builtins_are_open() {
                step(&mut state, TemplateLibraryChainStep::Unknown);
            } else {
                for library in backend
                    .builtin_indices
                    .iter()
                    .filter_map(|index| self.libraries.get(*index))
                {
                    step(&mut state, TemplateLibraryChainStep::Library(library));
                }
            }
            for loaded_name in loaded_names {
                let loaded_name = loaded_name.trim();
                if loaded_name.is_empty() || loaded_name.chars().any(char::is_whitespace) {
                    continue;
                }
                match backend.loadable_by_name.get(loaded_name) {
                    Some(TemplateLibraryIndexEntry::Resolved(index)) => {
                        if let Some(library) = self.libraries.get(*index) {
                            step(&mut state, TemplateLibraryChainStep::Library(library));
                        }
                    }
                    Some(TemplateLibraryIndexEntry::Unresolved { .. }) => {
                        step(&mut state, TemplateLibraryChainStep::Unknown);
                    }
                    None if backend.load_name_is_open_str(loaded_name) => {
                        step(&mut state, TemplateLibraryChainStep::Unknown);
                    }
                    None => {}
                }
            }
            finish(state);
        }
        if !visited || view.has_omissions() {
            let mut state = initial();
            step(&mut state, TemplateLibraryChainStep::Unknown);
            finish(state);
        }
    }

    #[must_use]
    pub fn empty_ref() -> &'static Self {
        static EMPTY: LazyLock<TemplateLibraryCatalog> =
            LazyLock::new(TemplateLibraryCatalog::default);
        &EMPTY
    }

    #[must_use]
    pub(crate) fn from_libraries(libraries: Vec<TemplateLibrary>) -> Self {
        Self::from_libraries_and_settings_cases(
            libraries,
            TemplateLibrarySettingsCases::Standalone {
                backend: TemplateBackendLibraries::default(),
                omissions: Vec::new(),
            },
        )
    }

    pub(crate) fn from_libraries_with_omissions(libraries: Vec<TemplateLibrary>) -> Self {
        Self::from_libraries_and_settings_cases(
            libraries,
            TemplateLibrarySettingsCases::Standalone {
                backend: TemplateBackendLibraries::default(),
                omissions: vec![TemplateSettingsOmission::Settings],
            },
        )
    }

    fn from_libraries_and_settings_cases(
        libraries: Vec<TemplateLibrary>,
        settings_cases: TemplateLibrarySettingsCases,
    ) -> Self {
        let mut inventory = Self {
            libraries: Vec::new(),
            definitions_by_name: BTreeMap::new(),
            loadable_by_name: BTreeMap::new(),
            settings_cases,
            available_in_app_by_name: BTreeMap::new(),
            issues: Vec::new(),
        };

        for library in libraries {
            inventory.insert_library(library);
        }
        inventory.rebuild_standalone_inventory();
        inventory.sort_and_dedup_available_in_app();
        inventory
    }

    pub(crate) fn set_testing_settings_cases(
        &mut self,
        settings_cases: Vec<Vec<TestingBackendSettings>>,
    ) {
        let identities = TemplateSettingsCases::for_testing(
            &settings_cases.iter().map(Vec::len).collect::<Vec<_>>(),
            false,
        );
        let known: Vec<_> = settings_cases
            .into_iter()
            .zip(identities.settings_cases())
            .map(|(backends, settings_case)| TemplateLibrarySettingsCase {
                id: settings_case.id(),
                slots: backends
                    .into_iter()
                    .zip(settings_case.backends())
                    .map(|((loadable, builtins), backend)| {
                        TemplateLibrarySlot::Backend(
                            backend.id(),
                            TemplateBackendLibraries {
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
                                                    TemplateLibraryKind::Loadable { load_name }
                                                        if load_name == &name
                                                ) && library.module_name() == &module
                                            })
                                            .unwrap_or_else(|| {
                                                panic!(
                                                    "configured test library {name} should resolve to {module}"
                                                )
                                            });
                                        (name, TemplateLibraryIndexEntry::Resolved(index))
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
                                app_names_after_remainder: BTreeSet::new(),
                                authoritative_names: BTreeSet::new(),
                                ..TemplateBackendLibraries::default()
                            },
                        )
                    })
                    .collect(),
                guidance: TemplateEvidenceCompleteness::Complete,
            })
            .collect();
        self.settings_cases.replace_known(known);
    }

    fn rebuild_standalone_inventory(&mut self) {
        let backend = TemplateBackendLibraries {
            loadable_by_name: self
                .loadable_by_name
                .iter()
                .map(|(name, index)| (name.clone(), TemplateLibraryIndexEntry::Resolved(*index)))
                .collect(),
            builtin_indices: self
                .libraries
                .iter()
                .enumerate()
                .filter_map(|(index, library)| {
                    matches!(&library.kind, TemplateLibraryKind::Builtin).then_some(index)
                })
                .collect(),
            app_names_after_remainder: BTreeSet::new(),
            authoritative_names: BTreeSet::new(),
            ..TemplateBackendLibraries::default()
        };
        let omissions = match &self.settings_cases {
            TemplateLibrarySettingsCases::Standalone { omissions, .. } => omissions.clone(),
            TemplateLibrarySettingsCases::Exhaustive(_)
            | TemplateLibrarySettingsCases::WithOmissions { .. } => return,
        };
        self.settings_cases = TemplateLibrarySettingsCases::Standalone { backend, omissions };
    }

    /// Whether discovery may have omitted definition names from the catalog index.
    #[must_use]
    pub(crate) fn definition_names_are_open(&self) -> bool {
        self.settings_cases.has_omissions()
            || self.settings_cases.has_unknown_loadables()
            || self
                .settings_cases
                .known()
                .iter()
                .any(|settings_case| settings_case.guidance.is_open())
            || self
                .resolved_libraries()
                .any(TemplateLibrary::symbols_are_unobserved)
            || !self.issues.is_empty()
    }

    pub(crate) fn inventory_symbol_names(
        &self,
        kind: TemplateSymbolKind,
    ) -> impl Iterator<Item = &str> + '_ {
        self.definitions_by_name
            .get(&kind)
            .into_iter()
            .flat_map(BTreeMap::keys)
            .map(String::as_str)
    }

    fn resolved_libraries(&self) -> impl Iterator<Item = &TemplateLibrary> + '_ {
        self.resolved_libraries_in_view(LibraryScopeView::project_inventory(self))
            .into_iter()
    }

    fn builtin_libraries(&self) -> impl Iterator<Item = &TemplateLibrary> + '_ {
        self.resolved_libraries()
            .filter(|library| matches!(&library.kind, TemplateLibraryKind::Builtin))
    }

    fn loadable_libraries(&self) -> impl Iterator<Item = (&LibraryName, &TemplateLibrary)> + '_ {
        self.resolved_libraries().filter_map(|library| {
            let TemplateLibraryKind::Loadable { load_name } = &library.kind else {
                return None;
            };
            Some((load_name, library))
        })
    }

    #[must_use]
    fn available_in_app_candidates(&self, name: &LibraryName) -> Vec<&TemplateLibrary> {
        self.available_in_app_by_name
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
            .available_in_app_by_name
            .values()
            .flatten()
            .filter_map(|index| self.libraries.get(*index))
            .filter(|library| library.symbol(kind, symbol_name).is_some())
            .collect();
        candidates.sort_by(|left, right| cmp_available_libraries(left, right));
        candidates
    }

    fn insert_library(&mut self, library: TemplateLibrary) -> usize {
        if let Some(index) = self
            .libraries
            .iter()
            .position(|existing| existing == &library)
        {
            match &library.kind {
                TemplateLibraryKind::Builtin => {}
                TemplateLibraryKind::Loadable { load_name } => {
                    self.loadable_by_name.insert(load_name.clone(), index);
                }
                TemplateLibraryKind::AvailableInApp { load_name, .. } => {
                    let indexes = self
                        .available_in_app_by_name
                        .entry(load_name.clone())
                        .or_default();
                    if !indexes.contains(&index) {
                        indexes.push(index);
                    }
                }
            }
            return index;
        }
        match &library.kind {
            TemplateLibraryKind::Builtin => self.push_library(library),
            TemplateLibraryKind::Loadable { load_name } => {
                let load_name = load_name.clone();
                let index = self.push_library(library);
                self.loadable_by_name.insert(load_name, index);
                index
            }
            TemplateLibraryKind::AvailableInApp { load_name, app } => {
                let load_name = load_name.clone();
                let app = app.clone();
                let module_name = library.module_name().clone();
                if let Some(existing_index) = self.libraries.iter().position(|existing| {
                    matches!(
                        &existing.kind,
                        TemplateLibraryKind::AvailableInApp {
                            load_name: existing_name,
                            app: existing_app,
                        } if existing_name == &load_name && existing_app == &app
                    ) && existing.module_name() == &module_name
                }) {
                    let indexes = self.available_in_app_by_name.entry(load_name).or_default();
                    if !indexes.contains(&existing_index) {
                        indexes.push(existing_index);
                    }
                    return existing_index;
                }

                let index = self.push_library(library);
                self.available_in_app_by_name
                    .entry(load_name)
                    .or_default()
                    .push(index);
                index
            }
        }
    }

    fn push_library(&mut self, library: TemplateLibrary) -> usize {
        let index = self.libraries.len();
        let names: Vec<_> = library
            .symbols()
            .iter()
            .map(|symbol| (symbol.kind, symbol.name().to_string()))
            .collect();
        self.libraries.push(library);
        for (kind, name) in names {
            self.definitions_by_name
                .entry(kind)
                .or_default()
                .entry(name)
                .or_default()
                .push(index);
        }
        index
    }

    fn add_configured_tag_definitions(&mut self, db: &dyn ProjectDb, project: Project) {
        let configured: Vec<_> = project
            .tagspecs(db)
            .libraries
            .iter()
            .flat_map(|library| {
                library
                    .tags
                    .iter()
                    .map(|tag| (library.module.as_str(), tag.name.as_str()))
            })
            .collect();
        for (module, name) in configured {
            for (index, library) in self.libraries.iter_mut().enumerate() {
                if library.module_name_str() == module && library.insert_configured_tag(name) {
                    self.definitions_by_name
                        .entry(TemplateSymbolKind::Tag)
                        .or_default()
                        .entry(name.to_string())
                        .or_default()
                        .push(index);
                }
            }
        }
    }

    fn insert_available_candidates(
        &mut self,
        db: &dyn ProjectDb,
        project: Project,
        loadable_template_library_modules: &BTreeSet<PythonModuleName>,
    ) {
        let mut excluded_modules: BTreeSet<_> = self
            .loadable_libraries()
            .map(|(_name, library)| library.module_name().clone())
            .chain(
                self.builtin_libraries()
                    .map(TemplateLibrary::module_name)
                    .cloned(),
            )
            .collect();

        excluded_modules.extend(loadable_template_library_modules.iter().cloned());

        let candidates = templatetag_candidates(db, project);
        if candidates.has_omissions() {
            self.issues.push(TemplateLibraryIssue::Discovery);
        }
        for candidate in candidates.candidates().iter().cloned() {
            if excluded_modules.contains(candidate.module.name()) {
                continue;
            }

            let id = TemplateLibraryId::new(
                db,
                Some(candidate.module.file()),
                candidate.module.name().clone(),
            );
            let facts = template_library_definition_facts(db, id);
            if facts.source_failed() {
                self.issues
                    .push(TemplateLibraryIssue::NamedSource(candidate.name.clone()));
                continue;
            }
            if facts.is_recovered() {
                self.issues
                    .push(TemplateLibraryIssue::NamedSource(candidate.name.clone()));
            }
            if facts.is_library() {
                let symbols = facts.symbols().cloned().collect();
                self.insert_library(TemplateLibrary::available_in_app(
                    id,
                    candidate.name.clone(),
                    candidate.app.clone(),
                    candidate.into_python_module(),
                    symbols,
                ));
            }
        }

        self.sort_and_dedup_available_in_app();
    }

    fn sort_and_dedup_available_in_app(&mut self) {
        let libraries = &self.libraries;
        for indexes in self.available_in_app_by_name.values_mut() {
            indexes.sort_by(
                |left, right| match (libraries.get(*left), libraries.get(*right)) {
                    (Some(left), Some(right)) => cmp_available_libraries(left, right),
                    (None, _) | (_, None) => Ordering::Equal,
                },
            );
            indexes.dedup_by(
                |left, right| match (libraries.get(*left), libraries.get(*right)) {
                    (Some(left), Some(right)) => same_available_in_app_library(left, right),
                    (None, _) | (_, None) => false,
                },
            );
        }
    }
}

// Keep the correlated settings/app/backend orchestration together: splitting it would require
// exposing its partially built library inventory and weaken locality of the ordering semantics.
#[allow(clippy::too_many_lines)]
#[salsa::tracked(returns(ref))]
pub fn template_library_catalog(db: &dyn ProjectDb, project: Project) -> TemplateLibraryCatalog {
    project.touch_search_path_roots(db);

    if settings_module_file(db, project).is_none() {
        if project.tagspecs(db).libraries.is_empty() {
            return TemplateLibraryCatalog::default();
        }

        // Explicit configured structural facts remain useful to source-only commands even when
        // there is no settings source or installed Django package to inspect. Model Django's
        // default builtin modules as configured-only libraries: they have keyed module identity,
        // but deliberately no source file or navigable origin.
        let mut libraries = TemplateLibraryCatalog::from_libraries(Vec::new());
        let backend =
            insert_backend_library_values(db, project, &[], &[], &BTreeMap::new(), &mut libraries);
        let TemplateLibrarySettingsCases::Standalone {
            backend: inventory, ..
        } = &mut libraries.settings_cases
        else {
            unreachable!("configured project inventory should be standalone")
        };
        *inventory = backend;
        libraries.add_configured_tag_definitions(db, project);
        return libraries;
    }

    let template_settings_cases = template_settings_cases(db, project);
    let mut libraries = TemplateLibraryCatalog::from_libraries(Vec::new());
    let mut loadable_template_library_modules = BTreeSet::new();

    let django_module = PythonModuleName::parse("django").expect("django is a valid module name");
    let (discovered, issues) = templatetag_package_libraries(db, project, &django_module);
    libraries.issues.extend(issues);
    let mut common_libraries = BTreeMap::new();
    insert_loadable_libraries(
        &mut libraries,
        &mut loadable_template_library_modules,
        &mut common_libraries,
        discovered,
    );

    let mut app_library_cases = Vec::new();
    for settings_case in template_settings_cases.settings_cases() {
        let installed_apps = settings_case.installed_apps();
        if app_library_cases
            .iter()
            .any(|existing: &InstalledAppLibraries| existing.evidence == installed_apps)
        {
            continue;
        }
        let mut app_libraries = common_libraries.clone();
        let mut app_remainder = false;
        let mut discovery_remainder = false;
        let mut app_names_after_remainder = BTreeSet::new();
        let mut unresolved_app_names = BTreeMap::new();
        for evidence in installed_apps.iter().cloned() {
            let InstalledAppEvidence::Known(installed_app) = evidence else {
                app_remainder = true;
                app_names_after_remainder.clear();
                continue;
            };
            let Some(package_module) =
                installed_app_package_module(db, project, &installed_app.value)
            else {
                discovery_remainder = true;
                app_names_after_remainder.clear();
                continue;
            };
            let (discovered, issues) = templatetag_package_libraries(db, project, &package_module);
            let discovered_names = discovered
                .iter()
                .map(|(load_name, _, _, _)| load_name)
                .collect::<BTreeSet<_>>();
            for load_name in &discovered_names {
                unresolved_app_names.remove(*load_name);
            }
            for issue in &issues {
                if let TemplateLibraryIssue::NamedSource(load_name) = issue
                    && !discovered_names.contains(load_name)
                {
                    unresolved_app_names
                        .insert(load_name.clone(), app_libraries.get(load_name).copied());
                }
            }
            let discovery_failed = issues
                .iter()
                .any(|issue| matches!(issue, TemplateLibraryIssue::Discovery));
            if discovery_failed {
                discovery_remainder = true;
                app_names_after_remainder.clear();
            } else if app_remainder || discovery_remainder {
                app_names_after_remainder
                    .extend(discovered.iter().map(|(name, _, _, _)| name.clone()));
            }
            libraries.issues.extend(
                issues
                    .into_iter()
                    .filter(|issue| !matches!(issue, TemplateLibraryIssue::Discovery)),
            );
            insert_loadable_libraries(
                &mut libraries,
                &mut loadable_template_library_modules,
                &mut app_libraries,
                discovered,
            );
        }
        app_library_cases.push(InstalledAppLibraries {
            evidence: installed_apps.to_vec(),
            libraries: app_libraries,
            app_remainder,
            discovery_remainder,
            names_after_remainder: app_names_after_remainder,
            unresolved_names: unresolved_app_names,
        });
    }

    let mut settings_cases = Vec::new();
    for settings_case in template_settings_cases.settings_cases() {
        let app_library_case = app_library_cases
            .iter()
            .find(|installed_apps| {
                installed_apps.evidence.as_slice() == settings_case.installed_apps()
            })
            .expect("every canonical settings case should have app evidence");
        let slots = settings_case
            .slots()
            .iter()
            .map(|slot| match *slot {
                TemplateBackendSlot::Backend(backend_id) => {
                    let backend = template_settings_cases
                        .backend(backend_id)
                        .expect("a canonical backend slot should resolve");
                    let mut backend_libraries = if backend.backend_name()
                        == Some("django.template.backends.django.DjangoTemplates")
                    {
                        insert_configured_backend_libraries(
                            db,
                            project,
                            backend,
                            &app_library_case.libraries,
                            &mut libraries,
                        )
                    } else if backend.backend_name().is_some()
                        && !backend.backend_completeness().is_open()
                    {
                        TemplateBackendLibraries::default()
                    } else {
                        TemplateBackendLibraries {
                            backend_completeness: TemplateEvidenceCompleteness::Open,
                            loadables_completeness: TemplateEvidenceCompleteness::Open,
                            builtins_completeness: TemplateEvidenceCompleteness::Open,
                            ..TemplateBackendLibraries::default()
                        }
                    };
                    for (load_name, known_candidate) in &app_library_case.unresolved_names {
                        if !backend_libraries.authoritative_names.contains(load_name) {
                            backend_libraries.loadable_by_name.insert(
                                load_name.clone(),
                                TemplateLibraryIndexEntry::Unresolved {
                                    known_candidate: *known_candidate,
                                },
                            );
                        }
                    }
                    backend_libraries.apps_completeness =
                        TemplateEvidenceCompleteness::open_if(app_library_case.app_remainder);
                    backend_libraries.discovery_completeness =
                        TemplateEvidenceCompleteness::open_if(app_library_case.discovery_remainder);
                    backend_libraries
                        .app_names_after_remainder
                        .clone_from(&app_library_case.names_after_remainder);
                    TemplateLibrarySlot::Backend(backend_id, backend_libraries)
                }
                TemplateBackendSlot::Remainder => TemplateLibrarySlot::Remainder,
            })
            .collect();
        settings_cases.push(TemplateLibrarySettingsCase {
            id: settings_case.id(),
            slots,
            guidance: TemplateEvidenceCompleteness::open_if(
                app_library_case.app_remainder || app_library_case.discovery_remainder,
            ),
        });
    }
    libraries.settings_cases.replace_known(settings_cases);
    libraries.insert_available_candidates(db, project, &loadable_template_library_modules);
    libraries.add_configured_tag_definitions(db, project);
    libraries
}

fn resolved_library_references(
    libraries: &BTreeMap<LibraryName, usize>,
) -> BTreeMap<LibraryName, TemplateLibraryIndexEntry> {
    libraries
        .iter()
        .map(|(name, index)| (name.clone(), TemplateLibraryIndexEntry::Resolved(*index)))
        .collect()
}

fn insert_configured_backend_libraries(
    db: &dyn ProjectDb,
    project: Project,
    backend: &TemplateBackendCase,
    app_libraries: &BTreeMap<LibraryName, usize>,
    libraries: &mut TemplateLibraryCatalog,
) -> TemplateBackendLibraries {
    let mut result = insert_backend_library_values(
        db,
        project,
        backend.libraries(),
        backend.builtins(),
        app_libraries,
        libraries,
    );
    result.backend_completeness =
        TemplateEvidenceCompleteness::open_if(backend.backend_completeness().is_open());
    result.loadables_completeness =
        TemplateEvidenceCompleteness::open_if(backend.libraries_completeness().is_open());
    result.builtins_completeness =
        TemplateEvidenceCompleteness::open_if(backend.builtins_completeness().is_open());
    result
}

fn insert_backend_library_values(
    db: &dyn ProjectDb,
    project: Project,
    configured_libraries: &[(String, PythonModuleName)],
    configured_builtins: &[PythonModuleName],
    app_libraries: &BTreeMap<LibraryName, usize>,
    libraries: &mut TemplateLibraryCatalog,
) -> TemplateBackendLibraries {
    let mut result = TemplateBackendLibraries {
        loadable_by_name: resolved_library_references(app_libraries),
        builtin_indices: Vec::new(),
        app_names_after_remainder: BTreeSet::new(),
        authoritative_names: BTreeSet::new(),
        ..TemplateBackendLibraries::default()
    };
    for (load_name, module_name) in configured_libraries {
        let Ok(load_name) = LibraryName::parse(load_name) else {
            result.loadables_completeness = TemplateEvidenceCompleteness::Open;
            continue;
        };
        result.authoritative_names.insert(load_name.clone());
        let library = match library_from_module_name(db, project, module_name.clone()) {
            ConfiguredLibraryModule::Source {
                id,
                module,
                symbols,
                recovered,
            } => {
                if recovered {
                    libraries
                        .issues
                        .push(TemplateLibraryIssue::NamedSource(load_name.clone()));
                }
                TemplateLibrary::loadable(id, load_name.clone(), module, symbols)
            }
            ConfiguredLibraryModule::SourceLess { id, module } => {
                TemplateLibrary::source_less_loadable(id, load_name.clone(), module)
            }
            ConfiguredLibraryModule::NotLibrary => {
                result.loadable_by_name.insert(
                    load_name,
                    TemplateLibraryIndexEntry::Unresolved {
                        known_candidate: None,
                    },
                );
                continue;
            }
        };
        let index = libraries.insert_library(library);
        result
            .loadable_by_name
            .insert(load_name, TemplateLibraryIndexEntry::Resolved(index));
    }

    let builtins = DEFAULT_TEMPLATE_BUILTINS
        .iter()
        .map(|name| PythonModuleName::parse(name).expect("default builtin is a valid module name"))
        .chain(configured_builtins.iter().cloned());
    for module_name in builtins {
        let library = match library_from_module_name(db, project, module_name) {
            ConfiguredLibraryModule::Source {
                id,
                module,
                symbols,
                recovered,
            } => {
                if recovered {
                    libraries.issues.push(TemplateLibraryIssue::BuiltinSource);
                }
                TemplateLibrary::builtin(id, module, symbols)
            }
            ConfiguredLibraryModule::SourceLess { id, module } => {
                TemplateLibrary::source_less_builtin(id, module)
            }
            ConfiguredLibraryModule::NotLibrary => {
                libraries.issues.push(TemplateLibraryIssue::BuiltinSource);
                continue;
            }
        };
        let index = libraries.insert_library(library);
        result.builtin_indices.push(index);
    }

    result
}

fn insert_loadable_libraries(
    libraries: &mut TemplateLibraryCatalog,
    loadable_modules: &mut BTreeSet<PythonModuleName>,
    loadable_by_name: &mut BTreeMap<LibraryName, usize>,
    discovered: Vec<DiscoveredLibrary>,
) {
    for (load_name, id, module, symbols) in discovered {
        loadable_modules.insert(module.name().clone());
        let index = libraries.insert_library(TemplateLibrary::loadable(
            id,
            load_name.clone(),
            module,
            symbols,
        ));
        loadable_by_name.insert(load_name, index);
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
        let id = TemplateLibraryId::new(
            db,
            Some(candidate.module.file()),
            candidate.module.name().clone(),
        );
        let facts = template_library_definition_facts(db, id);
        if facts.source_failed() {
            issues.push(TemplateLibraryIssue::NamedSource(candidate.name.clone()));
            continue;
        }
        if facts.is_recovered() {
            issues.push(TemplateLibraryIssue::NamedSource(candidate.name.clone()));
        }
        if facts.is_library() {
            libraries.push((
                candidate.name.clone(),
                id,
                candidate.into_python_module(),
                facts.symbols().cloned().collect(),
            ));
        }
    }

    (libraries, issues)
}

fn library_from_module_name(
    db: &dyn ProjectDb,
    project: Project,
    module_name: PythonModuleName,
) -> ConfiguredLibraryModule {
    let Some(module) = PythonSourceModule::resolve(db, project, module_name.clone()) else {
        return ConfiguredLibraryModule::SourceLess {
            id: TemplateLibraryId::new(db, None, module_name.clone()),
            module: module_name,
        };
    };
    let id = TemplateLibraryId::new(db, Some(module.file()), module.name().clone());
    let facts = template_library_definition_facts(db, id);
    if facts.is_library() {
        ConfiguredLibraryModule::Source {
            id,
            module,
            symbols: facts.symbols().cloned().collect(),
            recovered: facts.is_recovered(),
        }
    } else {
        ConfiguredLibraryModule::NotLibrary
    }
}

fn cmp_available_libraries(left: &TemplateLibrary, right: &TemplateLibrary) -> Ordering {
    left.available_in_app_module()
        .cmp(&right.available_in_app_module())
        .then_with(|| left.load_name().cmp(&right.load_name()))
        .then_with(|| left.module_name_str().cmp(right.module_name_str()))
}

fn same_available_in_app_library(left: &TemplateLibrary, right: &TemplateLibrary) -> bool {
    let (Some(left_app), Some(right_app)) = (
        left.available_in_app_module(),
        right.available_in_app_module(),
    ) else {
        return false;
    };

    left_app == right_app && left.module_name_str() == right.module_name_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_lookup_ignores_unselected_settings_case_omissions() {
        let settings_cases = TemplateSettingsCases::for_testing(&[1], false);
        let settings_case = &settings_cases.settings_cases()[0];
        let backend = settings_case.backends()[0].id();
        let libraries = TemplateLibraryCatalog {
            settings_cases: TemplateLibrarySettingsCases::WithOmissions {
                known: vec![TemplateLibrarySettingsCase {
                    id: settings_case.id(),
                    slots: vec![TemplateLibrarySlot::Backend(
                        backend,
                        TemplateBackendLibraries::default(),
                    )],
                    guidance: TemplateEvidenceCompleteness::Complete,
                }],
                omissions: vec![TemplateSettingsOmission::Settings],
            },
            ..TemplateLibraryCatalog::default()
        };
        let name = LibraryName::parse("missing").unwrap();
        let scoped =
            TemplateBackendScope::selected(vec![TemplateBackendSelection::Backend(backend)])
                .unwrap();

        assert_eq!(
            libraries.loadable_library_in_scope(&scoped, &name),
            LoadableLibraryLookup::Absent
        );
        assert_eq!(
            libraries.scoped_symbol_lookup_in_scope(&scoped, "missing", TemplateSymbolKind::Tag,),
            ScopedTemplateSymbolLookup::Absent
        );
        assert_eq!(
            libraries.template_symbol_lookup_in_scope(&scoped, "missing", TemplateSymbolKind::Tag,),
            AppTemplateSymbolLookup::Absent
        );
        assert_eq!(
            libraries.missing_library_lookup_in_scope(&scoped, &name),
            MissingTemplateLibraryLookup::Absent
        );
        assert_eq!(
            libraries.effective_definition_libraries_in_scope(
                &scoped,
                "missing",
                TemplateSymbolKind::Tag,
                &[],
            ),
            [EffectiveDefinitionLibrary::Known(None)]
        );
        let chains = libraries.library_chains_in_scope(&scoped, &[]);
        assert_eq!(chains.len(), 1);
        assert!(chains[0].steps().is_empty());
        assert_eq!(
            libraries.loadable_library_in_scope(&TemplateBackendScope::project_inventory(), &name,),
            LoadableLibraryLookup::Inconclusive(Vec::new())
        );
    }
}

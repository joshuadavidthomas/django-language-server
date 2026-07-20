use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::slice::Iter;
use std::sync::LazyLock;

use djls_source::File;

use super::candidates::templatetag_candidates;
use super::candidates::templatetag_candidates_in_package;
use super::configurations::TemplateBackendConfiguration;
use super::configurations::TemplateBackendId;
use super::configurations::TemplateConfigurationId;
use super::configurations::TemplateConfigurationSlot;
use super::configurations::TemplateConfigurations;
use super::configurations::template_configurations;
use super::installed_app_package_module;
use super::names::LibraryName;
use super::names::TemplateSymbolName;
use super::registrations::template_library_definition_facts;
use super::resolution::TemplateBackendScope;
use super::resolution::TemplateBackendScopeKind;
use super::resolution::TemplateBackendSelection;
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
    Installed {
        load_name: LibraryName,
    },
    Available {
        load_name: LibraryName,
        app: PythonModuleName,
    },
}

/// Stable interned identity for one Template Library module.
///
/// Configured-only libraries have no source file. Keeping that absence in the identity prevents
/// configuration evidence from masquerading as a navigable Python source.
#[salsa::interned(no_lifetime, debug)]
pub struct TemplateLibraryKey {
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
enum SymbolInventory {
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
    key: TemplateLibraryKey,
    module: TemplateLibraryModule,
    kind: TemplateLibraryKind,
    symbol_inventory: SymbolInventory,
    symbols: Vec<TemplateSymbol>,
    tag_symbols: BTreeMap<String, usize>,
    filter_symbols: BTreeMap<String, usize>,
}

impl TemplateLibrary {
    fn new(
        key: TemplateLibraryKey,
        module: TemplateLibraryModule,
        kind: TemplateLibraryKind,
        symbol_inventory: SymbolInventory,
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
            key,
            module,
            kind,
            symbol_inventory,
            symbols,
            tag_symbols,
            filter_symbols,
        }
    }

    #[must_use]
    fn builtin(
        key: TemplateLibraryKey,
        module: PythonSourceModule,
        symbols: Vec<TemplateSymbol>,
    ) -> Self {
        Self::new(
            key,
            TemplateLibraryModule::Source(module),
            TemplateLibraryKind::Builtin,
            SymbolInventory::Observed,
            symbols,
        )
    }

    #[must_use]
    pub(crate) fn configured_builtin(
        key: TemplateLibraryKey,
        module: PythonModuleName,
        symbols: Vec<TemplateSymbol>,
    ) -> Self {
        Self::new(
            key,
            TemplateLibraryModule::Configured(module),
            TemplateLibraryKind::Builtin,
            SymbolInventory::Observed,
            symbols,
        )
    }

    #[must_use]
    fn source_less_builtin(key: TemplateLibraryKey, module: PythonModuleName) -> Self {
        Self::new(
            key,
            TemplateLibraryModule::Configured(module),
            TemplateLibraryKind::Builtin,
            SymbolInventory::Unobserved,
            Vec::new(),
        )
    }

    #[must_use]
    fn installed(
        key: TemplateLibraryKey,
        load_name: LibraryName,
        module: PythonSourceModule,
        symbols: Vec<TemplateSymbol>,
    ) -> Self {
        Self::new(
            key,
            TemplateLibraryModule::Source(module),
            TemplateLibraryKind::Installed { load_name },
            SymbolInventory::Observed,
            symbols,
        )
    }

    #[must_use]
    pub(crate) fn configured_installed(
        key: TemplateLibraryKey,
        load_name: LibraryName,
        module: PythonModuleName,
        symbols: Vec<TemplateSymbol>,
    ) -> Self {
        Self::new(
            key,
            TemplateLibraryModule::Configured(module),
            TemplateLibraryKind::Installed { load_name },
            SymbolInventory::Observed,
            symbols,
        )
    }

    #[must_use]
    fn source_less_installed(
        key: TemplateLibraryKey,
        load_name: LibraryName,
        module: PythonModuleName,
    ) -> Self {
        Self::new(
            key,
            TemplateLibraryModule::Configured(module),
            TemplateLibraryKind::Installed { load_name },
            SymbolInventory::Unobserved,
            Vec::new(),
        )
    }

    #[must_use]
    pub(crate) fn configured_available(
        key: TemplateLibraryKey,
        load_name: LibraryName,
        app: PythonModuleName,
        module: PythonModuleName,
        symbols: Vec<TemplateSymbol>,
    ) -> Self {
        Self::new(
            key,
            TemplateLibraryModule::Configured(module),
            TemplateLibraryKind::Available { load_name, app },
            SymbolInventory::Observed,
            symbols,
        )
    }

    #[must_use]
    fn available(
        key: TemplateLibraryKey,
        load_name: LibraryName,
        app: PythonModuleName,
        module: PythonSourceModule,
        symbols: Vec<TemplateSymbol>,
    ) -> Self {
        Self::new(
            key,
            TemplateLibraryModule::Source(module),
            TemplateLibraryKind::Available { load_name, app },
            SymbolInventory::Observed,
            symbols,
        )
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

    /// Return the resolved Python source when one was observed.
    #[must_use]
    pub fn source_file(&self) -> Option<File> {
        self.module.file()
    }

    #[must_use]
    pub fn key(&self) -> TemplateLibraryKey {
        self.key
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
    pub fn symbol_inventory_is_open(&self) -> bool {
        matches!(self.symbol_inventory, SymbolInventory::Unobserved)
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

/// Consumer-shaped certainty for a tag or filter in the selected template environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnvironmentSymbolLookup {
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
pub enum ContextualLibraryStep<'a> {
    Library(&'a TemplateLibrary),
    Unknown,
}

/// Builtins followed by loaded libraries for one feasible backend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextualLibraryChain<'a>(Vec<ContextualLibraryStep<'a>>);

impl<'a> ContextualLibraryChain<'a> {
    #[must_use]
    pub fn steps(&self) -> &[ContextualLibraryStep<'a>] {
        &self.0
    }
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TemplateLibrarySlot {
    Backend(TemplateBackendId, TemplateBackendLibraries),
    Remainder,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TemplateLibraryConfiguration {
    id: TemplateConfigurationId,
    slots: Vec<TemplateLibrarySlot>,
    guidance: KnowledgeState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TemplateLibraryConfigurations {
    Standalone {
        backend: TemplateBackendLibraries,
        omissions: Vec<TemplateConfigurationOmission>,
    },
    Exhaustive(Vec<TemplateLibraryConfiguration>),
    WithOmissions {
        known: Vec<TemplateLibraryConfiguration>,
        omissions: Vec<TemplateConfigurationOmission>,
    },
}

impl TemplateLibraryConfigurations {
    fn known(&self) -> &[TemplateLibraryConfiguration] {
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
            backend.loadables_state.is_open()
                || backend.apps_state.is_open()
                || backend.discovery_state.is_open()
        }) || self.known().iter().any(|configuration| {
            configuration.slots.iter().any(|slot| match slot {
                TemplateLibrarySlot::Backend(_, backend) => {
                    backend.loadables_state.is_open()
                        || backend.apps_state.is_open()
                        || backend.discovery_state.is_open()
                }
                TemplateLibrarySlot::Remainder => true,
            })
        })
    }

    fn replace_known(&mut self, known: Vec<TemplateLibraryConfiguration>) {
        *self = if self.has_omissions() {
            Self::WithOmissions {
                known,
                omissions: vec![TemplateConfigurationOmission::Settings],
            }
        } else {
            Self::Exhaustive(known)
        };
    }

    fn backend(
        &self,
        id: TemplateBackendId,
    ) -> Option<(&TemplateLibraryConfiguration, &TemplateBackendLibraries)> {
        self.known().iter().find_map(|configuration| {
            configuration.slots.iter().find_map(|slot| match slot {
                TemplateLibrarySlot::Backend(candidate, backend) if *candidate == id => {
                    Some((configuration, backend))
                }
                TemplateLibrarySlot::Backend(_, _) | TemplateLibrarySlot::Remainder => None,
            })
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TemplateLibraryReference {
    Resolved(usize),
    Unresolved { known_candidate: Option<usize> },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum KnowledgeState {
    #[default]
    Complete,
    Open,
}

impl KnowledgeState {
    const fn open_if(condition: bool) -> Self {
        if condition {
            Self::Open
        } else {
            Self::Complete
        }
    }

    const fn is_open(self) -> bool {
        matches!(self, Self::Open)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TemplateBackendLibraries {
    loadable_by_name: BTreeMap<LibraryName, TemplateLibraryReference>,
    builtin_indices: Vec<usize>,
    /// Uncertainty about this ID-bearing backend's type or backend-specific behavior.
    backend_state: KnowledgeState,
    /// Uncertainty is owned by the backend field that produced it. This keeps an unrelated
    /// backend or sibling field from weakening an otherwise exact lookup after file narrowing.
    loadables_state: KnowledgeState,
    builtins_state: KnowledgeState,
    apps_state: KnowledgeState,
    discovery_state: KnowledgeState,
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

        self.loadables_state.is_open()
            || ((self.apps_state.is_open() || self.discovery_state.is_open())
                && !self.app_names_after_remainder.contains(name))
    }
}

type TestingBackendConfiguration = (Vec<(LibraryName, PythonModuleName)>, Vec<PythonModuleName>);
type DiscoveredLibrary = (
    LibraryName,
    TemplateLibraryKey,
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
        key: TemplateLibraryKey,
        module: PythonSourceModule,
        symbols: Vec<TemplateSymbol>,
        recovered: bool,
    },
    SourceLess {
        key: TemplateLibraryKey,
        module: PythonModuleName,
    },
    NotLibrary,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateLibraries {
    libraries: Vec<TemplateLibrary>,
    definitions_by_name: BTreeMap<TemplateSymbolKind, BTreeMap<String, Vec<usize>>>,
    installed_by_name: BTreeMap<LibraryName, usize>,
    configurations: TemplateLibraryConfigurations,
    available_by_name: BTreeMap<LibraryName, Vec<usize>>,
    issues: Vec<TemplateLibraryIssue>,
}

impl Default for TemplateLibraries {
    fn default() -> Self {
        Self {
            libraries: Vec::new(),
            definitions_by_name: BTreeMap::new(),
            installed_by_name: BTreeMap::new(),
            configurations: TemplateLibraryConfigurations::Standalone {
                backend: TemplateBackendLibraries::default(),
                omissions: vec![TemplateConfigurationOmission::Settings],
            },
            available_by_name: BTreeMap::new(),
            issues: Vec::new(),
        }
    }
}

#[derive(Clone, Copy)]
enum BackendAlternative<'a> {
    Known {
        backend: &'a TemplateBackendLibraries,
        guidance: KnowledgeState,
    },
    NoBackend {
        guidance: KnowledgeState,
    },
    Unknown,
}

impl<'a> BackendAlternative<'a> {
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
            Self::Known { backend, .. } => backend.backend_state.is_open(),
            Self::NoBackend { .. } => false,
            Self::Unknown => true,
        }
    }

    fn builtins_are_open(self) -> bool {
        match self {
            Self::Known { backend, .. } => backend.builtins_state.is_open(),
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

    fn loadable(self, name: &LibraryName) -> Option<TemplateLibraryReference> {
        self.backend()?.loadable_by_name.get(name).copied()
    }

    fn has_open_inventory(self) -> bool {
        self.guidance_is_open()
            || self.backend_is_open()
            || self.backend().is_some_and(|backend| {
                backend.loadables_state.is_open()
                    || backend.apps_state.is_open()
                    || backend.discovery_state.is_open()
            })
    }
}

#[derive(Clone, Copy)]
struct AlternativeView<'a> {
    libraries: &'a TemplateLibraries,
    scope: &'a TemplateBackendScope,
}

impl<'a> AlternativeView<'a> {
    const fn new(libraries: &'a TemplateLibraries, scope: &'a TemplateBackendScope) -> Self {
        Self { libraries, scope }
    }

    fn project_inventory(libraries: &'a TemplateLibraries) -> Self {
        Self::new(libraries, TemplateBackendScope::project_inventory_ref())
    }

    fn alternatives(self) -> AlternativeIter<'a> {
        let kind = match self.scope.kind() {
            TemplateBackendScopeKind::ProjectInventory => AlternativeIterKind::ProjectInventory {
                standalone: self.libraries.configurations.standalone(),
                configurations: self.libraries.configurations.known().iter(),
                current: None,
            },
            TemplateBackendScopeKind::Selected(selections) => {
                AlternativeIterKind::Scoped(selections.as_slice().iter())
            }
        };
        AlternativeIter {
            libraries: self.libraries,
            kind,
        }
    }

    fn has_omissions(self) -> bool {
        matches!(
            self.scope.kind(),
            TemplateBackendScopeKind::ProjectInventory
        ) && self.libraries.configurations.has_omissions()
    }

    fn is_selected(self) -> bool {
        matches!(self.scope.kind(), TemplateBackendScopeKind::Selected(_))
    }
}

struct CurrentConfiguration<'a> {
    configuration: &'a TemplateLibraryConfiguration,
    slots: Iter<'a, TemplateLibrarySlot>,
}

enum AlternativeIterKind<'a> {
    ProjectInventory {
        standalone: Option<&'a TemplateBackendLibraries>,
        configurations: Iter<'a, TemplateLibraryConfiguration>,
        current: Option<CurrentConfiguration<'a>>,
    },
    Scoped(Iter<'a, TemplateBackendSelection>),
}

struct AlternativeIter<'a> {
    libraries: &'a TemplateLibraries,
    kind: AlternativeIterKind<'a>,
}

impl<'a> Iterator for AlternativeIter<'a> {
    type Item = BackendAlternative<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.kind {
            AlternativeIterKind::ProjectInventory {
                standalone,
                configurations,
                current,
            } => loop {
                if let Some(backend) = standalone.take() {
                    return Some(BackendAlternative::Known {
                        backend,
                        guidance: KnowledgeState::Complete,
                    });
                }
                if let Some(state) = current {
                    if let Some(slot) = state.slots.next() {
                        return Some(match slot {
                            TemplateLibrarySlot::Backend(_, backend) => BackendAlternative::Known {
                                backend,
                                guidance: state.configuration.guidance,
                            },
                            TemplateLibrarySlot::Remainder => BackendAlternative::Unknown,
                        });
                    }
                    *current = None;
                }

                let configuration = configurations.next()?;
                if configuration.slots.is_empty() {
                    return Some(BackendAlternative::NoBackend {
                        guidance: configuration.guidance,
                    });
                }
                *current = Some(CurrentConfiguration {
                    configuration,
                    slots: configuration.slots.iter(),
                });
            },
            AlternativeIterKind::Scoped(selections) => {
                let selection = selections.next()?;
                Some(match *selection {
                    TemplateBackendSelection::Backend(backend) => {
                        self.libraries.configurations.backend(backend).map_or(
                            BackendAlternative::Unknown,
                            |(configuration, backend)| BackendAlternative::Known {
                                backend,
                                guidance: configuration.guidance,
                            },
                        )
                    }
                    TemplateBackendSelection::ConfigurationRemainder(_) => {
                        BackendAlternative::Unknown
                    }
                })
            }
        }
    }
}

impl TemplateLibraries {
    pub(crate) fn loadable_library_in_scope<'a>(
        &'a self,
        scope: &'a TemplateBackendScope,
        name: &LibraryName,
    ) -> LoadableLibraryLookup<'a> {
        self.loadable_library_in_view(AlternativeView::new(self, scope), name)
    }

    fn loadable_library_in_view<'a>(
        &'a self,
        view: AlternativeView<'a>,
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
                Some(TemplateLibraryReference::Resolved(index)) => {
                    matches.push(index);
                    if !indexes.contains(&index) {
                        indexes.push(index);
                    }
                }
                Some(TemplateLibraryReference::Unresolved { known_candidate }) => {
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
        Self::completion_library_names_in_view(AlternativeView::new(self, scope))
    }

    fn completion_library_names_in_view(view: AlternativeView<'_>) -> Vec<LibraryName> {
        view.alternatives()
            .filter_map(BackendAlternative::backend)
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
        self.resolved_libraries_in_view(AlternativeView::new(self, scope))
    }

    fn resolved_libraries_in_view(&self, view: AlternativeView<'_>) -> Vec<&TemplateLibrary> {
        let mut indexes = Vec::new();
        for backend in view.alternatives().filter_map(BackendAlternative::backend) {
            for index in backend
                .loadable_by_name
                .values()
                .filter_map(|reference| match reference {
                    TemplateLibraryReference::Resolved(index) => Some(*index),
                    TemplateLibraryReference::Unresolved { known_candidate } => *known_candidate,
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

    pub(crate) fn contextual_symbol_candidates_in_scope(
        &self,
        scope: &TemplateBackendScope,
        name: &str,
        kind: TemplateSymbolKind,
    ) -> Vec<TemplateSymbolCandidate> {
        self.contextual_symbol_candidates_in_view(AlternativeView::new(self, scope), name, kind)
    }

    fn contextual_symbol_candidates_in_view(
        &self,
        view: AlternativeView<'_>,
        name: &str,
        kind: TemplateSymbolKind,
    ) -> Vec<TemplateSymbolCandidate> {
        let lookup = self.environment_symbol_lookup_in_view(view, name, kind);
        let libraries = self.resolved_libraries_in_view(view);
        let mut candidates = Vec::new();

        if matches!(lookup, EnvironmentSymbolLookup::Builtin) {
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

        let EnvironmentSymbolLookup::RequiresLoad(required_names) = lookup else {
            return candidates;
        };
        for library in libraries
            .into_iter()
            .filter(|library| matches!(&library.kind, TemplateLibraryKind::Installed { .. }))
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

    pub(crate) fn environment_symbol_lookup_in_scope(
        &self,
        scope: &TemplateBackendScope,
        name: &str,
        kind: TemplateSymbolKind,
    ) -> EnvironmentSymbolLookup {
        self.environment_symbol_lookup_in_view(AlternativeView::new(self, scope), name, kind)
    }

    fn environment_symbol_lookup_in_view(
        &self,
        view: AlternativeView<'_>,
        name: &str,
        kind: TemplateSymbolKind,
    ) -> EnvironmentSymbolLookup {
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
                    open |= library.symbol_inventory_is_open();
                }
            }
            builtin_present |= present;
            builtin_absent |= !present && !open;
            inconclusive |= !present && open;
        }
        if builtin_present && !builtin_absent && !inconclusive {
            return EnvironmentSymbolLookup::Builtin;
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
                    Some(TemplateLibraryReference::Resolved(index)) => {
                        if let Some(library) = self.libraries.get(index) {
                            let has_symbol = library.symbol(kind, name).is_some();
                            present |= has_symbol;
                            open |= !has_symbol && library.symbol_inventory_is_open();
                            absent |= !has_symbol && !library.symbol_inventory_is_open();
                        }
                    }
                    Some(TemplateLibraryReference::Unresolved { .. }) => open = true,
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
            EnvironmentSymbolLookup::Inconclusive
        } else if !required.is_empty() {
            EnvironmentSymbolLookup::RequiresLoad(required)
        } else if view.has_omissions() {
            EnvironmentSymbolLookup::Inconclusive
        } else {
            EnvironmentSymbolLookup::Absent
        }
    }

    pub(crate) fn template_symbol_lookup_in_scope(
        &self,
        scope: &TemplateBackendScope,
        name: &str,
        kind: TemplateSymbolKind,
    ) -> TemplateSymbolLookup {
        self.template_symbol_lookup_in_view(AlternativeView::new(self, scope), name, kind)
    }

    fn template_symbol_lookup_in_view(
        &self,
        view: AlternativeView<'_>,
        name: &str,
        kind: TemplateSymbolKind,
    ) -> TemplateSymbolLookup {
        let candidates = self.available_symbol_candidates(name, kind);
        let has_candidates = !candidates.is_empty();
        let mut has_uncertain_candidate = false;
        for candidate in candidates {
            let (Some(app), Some(load_name)) = (candidate.available_app(), candidate.load_name())
            else {
                continue;
            };
            let mut shadowed = false;
            let mut unshadowed = false;
            let mut open = view
                .alternatives()
                .any(BackendAlternative::guidance_is_open);
            for alternative in view.alternatives() {
                let Some(backend) = alternative.backend() else {
                    match alternative {
                        BackendAlternative::NoBackend { .. } => unshadowed = true,
                        BackendAlternative::Unknown => open = true,
                        BackendAlternative::Known { .. } => unreachable!(),
                    }
                    continue;
                };
                if backend.authoritative_names.contains(load_name) {
                    shadowed = true;
                    if let Some(TemplateLibraryReference::Resolved(index)) =
                        backend.loadable_by_name.get(load_name)
                        && self.libraries.get(*index).is_some_and(|library| {
                            library.symbol(kind, name).is_none()
                                && library.symbol_inventory_is_open()
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
                return TemplateSymbolLookup::FoundInApp {
                    app: app.clone(),
                    load_name: load_name.clone(),
                };
            }
        }
        if has_uncertain_candidate {
            TemplateSymbolLookup::Inconclusive
        } else if has_candidates {
            TemplateSymbolLookup::Absent
        } else if view.has_omissions()
            || view
                .alternatives()
                .any(BackendAlternative::has_open_inventory)
            || !self.issues.is_empty()
        {
            TemplateSymbolLookup::Inconclusive
        } else {
            TemplateSymbolLookup::Absent
        }
    }

    pub(crate) fn missing_library_lookup_in_scope(
        &self,
        scope: &TemplateBackendScope,
        name: &LibraryName,
    ) -> MissingLibraryLookup {
        self.missing_library_lookup_in_view(AlternativeView::new(self, scope), name)
    }

    fn missing_library_lookup_in_view(
        &self,
        view: AlternativeView<'_>,
        name: &LibraryName,
    ) -> MissingLibraryLookup {
        match self.loadable_library_in_view(view, name) {
            LoadableLibraryLookup::Found(_) | LoadableLibraryLookup::Ambiguous(_) => {
                return MissingLibraryLookup::Inconclusive;
            }
            LoadableLibraryLookup::Inconclusive(candidates)
                if !candidates.is_empty()
                    || view.alternatives().any(|backend| {
                        backend.load_name_is_open(name)
                            || matches!(
                                backend.loadable(name),
                                Some(TemplateLibraryReference::Unresolved { .. })
                            )
                    }) =>
            {
                return MissingLibraryLookup::Inconclusive;
            }
            LoadableLibraryLookup::Inconclusive(_) | LoadableLibraryLookup::Absent => {}
        }
        let candidates = self.available_library_candidates(name);
        if !candidates.is_empty() {
            if view.alternatives().any(|alternative| {
                alternative.guidance_is_open()
                    || match alternative {
                        BackendAlternative::Known { backend, .. } => {
                            !backend.authoritative_names.contains(name)
                                && backend.load_name_is_open(name)
                        }
                        BackendAlternative::NoBackend { .. } => false,
                        BackendAlternative::Unknown => true,
                    }
            }) {
                return MissingLibraryLookup::Inconclusive;
            }
            let mut apps: Vec<_> = candidates
                .iter()
                .filter_map(|candidate| candidate.available_app().cloned())
                .collect();
            apps.dedup();
            if !apps.is_empty() {
                return MissingLibraryLookup::FoundInApps(AvailableAppCandidates(apps));
            }
        }
        if view.has_omissions()
            || view
                .alternatives()
                .any(BackendAlternative::has_open_inventory)
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
            AlternativeView::new(self, scope),
            symbol_name,
            kind,
            loaded_names,
            visitor,
        );
    }

    fn for_each_effective_definition_library_in_view<'a>(
        &'a self,
        view: AlternativeView<'a>,
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
                    BackendAlternative::NoBackend { .. } => EffectiveDefinitionLibrary::Known(None),
                    BackendAlternative::Unknown => EffectiveDefinitionLibrary::Unknown,
                    BackendAlternative::Known { .. } => unreachable!(),
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
                } else if library.symbol_inventory_is_open() {
                    unobserved = Some(library);
                }
            }

            for loaded_name in loaded_names {
                let loaded_name = loaded_name.trim();
                if loaded_name.is_empty() || loaded_name.chars().any(char::is_whitespace) {
                    continue;
                }
                match backend.loadable_by_name.get(loaded_name) {
                    Some(TemplateLibraryReference::Resolved(index)) => {
                        if let Some(library) = self.libraries.get(*index) {
                            if has_symbol(library) {
                                effective = Some(library);
                                unobserved = None;
                                unknown = false;
                            } else if library.symbol_inventory_is_open() {
                                unobserved = Some(library);
                                unknown = false;
                            }
                        }
                    }
                    Some(TemplateLibraryReference::Unresolved { .. }) => {
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

    pub(crate) fn contextual_library_chains_in_scope<'a>(
        &'a self,
        scope: &'a TemplateBackendScope,
        loaded_names: &[&str],
    ) -> Vec<ContextualLibraryChain<'a>> {
        let mut chains = Vec::new();
        self.fold_contextual_library_chains_in_scope(
            scope,
            loaded_names,
            Vec::new,
            Vec::push,
            |steps| chains.push(ContextualLibraryChain(steps)),
        );
        chains
    }

    pub(crate) fn fold_contextual_library_chains_in_scope<'a, State>(
        &'a self,
        scope: &'a TemplateBackendScope,
        loaded_names: &[&str],
        initial: impl FnMut() -> State,
        step: impl FnMut(&mut State, ContextualLibraryStep<'a>),
        finish: impl FnMut(State),
    ) {
        self.fold_contextual_library_chains_in_view(
            AlternativeView::new(self, scope),
            loaded_names,
            initial,
            step,
            finish,
        );
    }

    fn fold_contextual_library_chains_in_view<'a, State>(
        &'a self,
        view: AlternativeView<'a>,
        loaded_names: &[&str],
        mut initial: impl FnMut() -> State,
        mut step: impl FnMut(&mut State, ContextualLibraryStep<'a>),
        mut finish: impl FnMut(State),
    ) {
        let mut visited = false;
        for alternative in view.alternatives() {
            visited = true;
            let mut state = initial();
            let Some(backend) = alternative.backend() else {
                match alternative {
                    BackendAlternative::NoBackend { .. } => {}
                    BackendAlternative::Unknown => {
                        step(&mut state, ContextualLibraryStep::Unknown);
                    }
                    BackendAlternative::Known { .. } => unreachable!(),
                }
                finish(state);
                continue;
            };
            let scoped = alternative;
            if scoped.backend_is_open() || scoped.builtins_are_open() {
                step(&mut state, ContextualLibraryStep::Unknown);
            } else {
                for library in backend
                    .builtin_indices
                    .iter()
                    .filter_map(|index| self.libraries.get(*index))
                {
                    step(&mut state, ContextualLibraryStep::Library(library));
                }
            }
            for loaded_name in loaded_names {
                let loaded_name = loaded_name.trim();
                if loaded_name.is_empty() || loaded_name.chars().any(char::is_whitespace) {
                    continue;
                }
                match backend.loadable_by_name.get(loaded_name) {
                    Some(TemplateLibraryReference::Resolved(index)) => {
                        if let Some(library) = self.libraries.get(*index) {
                            step(&mut state, ContextualLibraryStep::Library(library));
                        }
                    }
                    Some(TemplateLibraryReference::Unresolved { .. }) => {
                        step(&mut state, ContextualLibraryStep::Unknown);
                    }
                    None if backend.load_name_is_open_str(loaded_name) => {
                        step(&mut state, ContextualLibraryStep::Unknown);
                    }
                    None => {}
                }
            }
            finish(state);
        }
        if !visited || view.has_omissions() {
            let mut state = initial();
            step(&mut state, ContextualLibraryStep::Unknown);
            finish(state);
        }
    }

    #[must_use]
    pub fn empty_ref() -> &'static Self {
        static EMPTY: LazyLock<TemplateLibraries> = LazyLock::new(TemplateLibraries::default);
        &EMPTY
    }

    #[must_use]
    pub(crate) fn from_libraries(libraries: Vec<TemplateLibrary>) -> Self {
        Self::from_libraries_and_configurations(
            libraries,
            TemplateLibraryConfigurations::Standalone {
                backend: TemplateBackendLibraries::default(),
                omissions: Vec::new(),
            },
        )
    }

    pub(crate) fn from_libraries_with_omissions(libraries: Vec<TemplateLibrary>) -> Self {
        Self::from_libraries_and_configurations(
            libraries,
            TemplateLibraryConfigurations::Standalone {
                backend: TemplateBackendLibraries::default(),
                omissions: vec![TemplateConfigurationOmission::Settings],
            },
        )
    }

    fn from_libraries_and_configurations(
        libraries: Vec<TemplateLibrary>,
        configurations: TemplateLibraryConfigurations,
    ) -> Self {
        let mut inventory = Self {
            libraries: Vec::new(),
            definitions_by_name: BTreeMap::new(),
            installed_by_name: BTreeMap::new(),
            configurations,
            available_by_name: BTreeMap::new(),
            issues: Vec::new(),
        };

        for library in libraries {
            inventory.insert_library(library);
        }
        inventory.rebuild_standalone_inventory();
        inventory.sort_and_dedup_available();
        inventory
    }

    pub(crate) fn set_testing_configurations(
        &mut self,
        configurations: Vec<Vec<TestingBackendConfiguration>>,
    ) {
        let identities = TemplateConfigurations::for_testing(
            &configurations.iter().map(Vec::len).collect::<Vec<_>>(),
            false,
        );
        let known: Vec<_> = configurations
            .into_iter()
            .zip(identities.configurations())
            .map(|(backends, configuration)| TemplateLibraryConfiguration {
                id: configuration.id(),
                slots: backends
                    .into_iter()
                    .zip(configuration.backends())
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
                                app_names_after_remainder: BTreeSet::new(),
                                authoritative_names: BTreeSet::new(),
                                ..TemplateBackendLibraries::default()
                            },
                        )
                    })
                    .collect(),
                guidance: KnowledgeState::Complete,
            })
            .collect();
        self.configurations.replace_known(known);
    }

    fn rebuild_standalone_inventory(&mut self) {
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
            app_names_after_remainder: BTreeSet::new(),
            authoritative_names: BTreeSet::new(),
            ..TemplateBackendLibraries::default()
        };
        let omissions = match &self.configurations {
            TemplateLibraryConfigurations::Standalone { omissions, .. } => omissions.clone(),
            TemplateLibraryConfigurations::Exhaustive(_)
            | TemplateLibraryConfigurations::WithOmissions { .. } => return,
        };
        self.configurations = TemplateLibraryConfigurations::Standalone { backend, omissions };
    }

    /// Whether discovery may have omitted definition names from the catalog index.
    #[must_use]
    pub(crate) fn definition_names_are_open(&self) -> bool {
        self.configurations.has_omissions()
            || self.configurations.has_unknown_loadables()
            || self
                .configurations
                .known()
                .iter()
                .any(|configuration| configuration.guidance.is_open())
            || self
                .resolved_libraries()
                .any(TemplateLibrary::symbol_inventory_is_open)
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
        self.resolved_libraries_in_view(AlternativeView::project_inventory(self))
            .into_iter()
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
                TemplateLibraryKind::Installed { load_name } => {
                    self.installed_by_name.insert(load_name.clone(), index);
                }
                TemplateLibraryKind::Available { load_name, .. } => {
                    let indexes = self.available_by_name.entry(load_name.clone()).or_default();
                    if !indexes.contains(&index) {
                        indexes.push(index);
                    }
                }
            }
            return index;
        }
        match &library.kind {
            TemplateLibraryKind::Builtin => self.push_library(library),
            TemplateLibraryKind::Installed { load_name } => {
                let load_name = load_name.clone();
                let index = self.push_library(library);
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

                let index = self.push_library(library);
                self.available_by_name
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

            let key = TemplateLibraryKey::new(
                db,
                Some(candidate.module.file()),
                candidate.module.name().clone(),
            );
            let facts = template_library_definition_facts(db, key);
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
                self.insert_library(TemplateLibrary::available(
                    key,
                    candidate.name.clone(),
                    candidate.app.clone(),
                    candidate.into_python_module(),
                    symbols,
                ));
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

// Keep the correlated settings/app/backend orchestration together: splitting it would require
// exposing its partially built library inventory and weaken locality of the ordering semantics.
#[allow(clippy::too_many_lines)]
#[salsa::tracked(returns(ref))]
pub fn template_libraries(db: &dyn ProjectDb, project: Project) -> TemplateLibraries {
    project.touch_search_path_roots(db);

    if settings_module_file(db, project).is_none() {
        if project.tagspecs(db).libraries.is_empty() {
            return TemplateLibraries::default();
        }

        // Explicit configured structural facts remain useful to source-only commands even when
        // there is no settings source or installed Django package to inspect. Model Django's
        // default builtin modules as configured-only libraries: they have keyed module identity,
        // but deliberately no source file or navigable origin.
        let mut libraries = TemplateLibraries::from_libraries(Vec::new());
        let backend =
            insert_backend_library_values(db, project, &[], &[], &BTreeMap::new(), &mut libraries);
        let TemplateLibraryConfigurations::Standalone {
            backend: inventory, ..
        } = &mut libraries.configurations
        else {
            unreachable!("configured project inventory should be standalone")
        };
        *inventory = backend;
        libraries.add_configured_tag_definitions(db, project);
        return libraries;
    }

    let template_configurations = template_configurations(db, project);
    let mut libraries = TemplateLibraries::from_libraries(Vec::new());
    let mut installed_template_library_modules = BTreeSet::new();

    let django_module = PythonModuleName::parse("django").expect("django is a valid module name");
    let (discovered, issues) = templatetag_package_libraries(db, project, &django_module);
    libraries.issues.extend(issues);
    let mut common_libraries = BTreeMap::new();
    insert_installed_libraries(
        &mut libraries,
        &mut installed_template_library_modules,
        &mut common_libraries,
        discovered,
    );

    let mut app_configurations = Vec::new();
    for configuration in template_configurations.configurations() {
        let installed_apps = configuration.installed_apps();
        if app_configurations
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
            insert_installed_libraries(
                &mut libraries,
                &mut installed_template_library_modules,
                &mut app_libraries,
                discovered,
            );
        }
        app_configurations.push(InstalledAppLibraries {
            evidence: installed_apps.to_vec(),
            libraries: app_libraries,
            app_remainder,
            discovery_remainder,
            names_after_remainder: app_names_after_remainder,
            unresolved_names: unresolved_app_names,
        });
    }

    let mut configurations = Vec::new();
    for configuration in template_configurations.configurations() {
        let app_configuration = app_configurations
            .iter()
            .find(|installed_apps| {
                installed_apps.evidence.as_slice() == configuration.installed_apps()
            })
            .expect("every canonical configuration should have app evidence");
        let slots = configuration
            .slots()
            .iter()
            .map(|slot| match *slot {
                TemplateConfigurationSlot::Backend(backend_id) => {
                    let backend = template_configurations
                        .backend(backend_id)
                        .expect("a canonical backend slot should resolve");
                    let mut backend_libraries = if backend.backend_name()
                        == Some("django.template.backends.django.DjangoTemplates")
                    {
                        insert_configured_backend_libraries(
                            db,
                            project,
                            backend,
                            &app_configuration.libraries,
                            &mut libraries,
                        )
                    } else if backend.backend_name().is_some() && !backend.backend_state().is_open()
                    {
                        TemplateBackendLibraries::default()
                    } else {
                        TemplateBackendLibraries {
                            backend_state: KnowledgeState::Open,
                            loadables_state: KnowledgeState::Open,
                            builtins_state: KnowledgeState::Open,
                            ..TemplateBackendLibraries::default()
                        }
                    };
                    for (load_name, known_candidate) in &app_configuration.unresolved_names {
                        if !backend_libraries.authoritative_names.contains(load_name) {
                            backend_libraries.loadable_by_name.insert(
                                load_name.clone(),
                                TemplateLibraryReference::Unresolved {
                                    known_candidate: *known_candidate,
                                },
                            );
                        }
                    }
                    backend_libraries.apps_state =
                        KnowledgeState::open_if(app_configuration.app_remainder);
                    backend_libraries.discovery_state =
                        KnowledgeState::open_if(app_configuration.discovery_remainder);
                    backend_libraries
                        .app_names_after_remainder
                        .clone_from(&app_configuration.names_after_remainder);
                    TemplateLibrarySlot::Backend(backend_id, backend_libraries)
                }
                TemplateConfigurationSlot::Remainder => TemplateLibrarySlot::Remainder,
            })
            .collect();
        configurations.push(TemplateLibraryConfiguration {
            id: configuration.id(),
            slots,
            guidance: KnowledgeState::open_if(
                app_configuration.app_remainder || app_configuration.discovery_remainder,
            ),
        });
    }
    libraries.configurations.replace_known(configurations);
    libraries.insert_available_candidates(db, project, &installed_template_library_modules);
    libraries.add_configured_tag_definitions(db, project);
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

fn insert_configured_backend_libraries(
    db: &dyn ProjectDb,
    project: Project,
    backend: &TemplateBackendConfiguration,
    app_libraries: &BTreeMap<LibraryName, usize>,
    libraries: &mut TemplateLibraries,
) -> TemplateBackendLibraries {
    let mut result = insert_backend_library_values(
        db,
        project,
        backend.libraries(),
        backend.builtins(),
        app_libraries,
        libraries,
    );
    result.backend_state = KnowledgeState::open_if(backend.backend_state().is_open());
    result.loadables_state = KnowledgeState::open_if(backend.libraries_state().is_open());
    result.builtins_state = KnowledgeState::open_if(backend.builtins_state().is_open());
    result
}

fn insert_backend_library_values(
    db: &dyn ProjectDb,
    project: Project,
    configured_libraries: &[(String, PythonModuleName)],
    configured_builtins: &[PythonModuleName],
    app_libraries: &BTreeMap<LibraryName, usize>,
    libraries: &mut TemplateLibraries,
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
            result.loadables_state = KnowledgeState::Open;
            continue;
        };
        result.authoritative_names.insert(load_name.clone());
        let library = match library_from_module_name(db, project, module_name.clone()) {
            ConfiguredLibraryModule::Source {
                key,
                module,
                symbols,
                recovered,
            } => {
                if recovered {
                    libraries
                        .issues
                        .push(TemplateLibraryIssue::NamedSource(load_name.clone()));
                }
                TemplateLibrary::installed(key, load_name.clone(), module, symbols)
            }
            ConfiguredLibraryModule::SourceLess { key, module } => {
                TemplateLibrary::source_less_installed(key, load_name.clone(), module)
            }
            ConfiguredLibraryModule::NotLibrary => {
                result.loadable_by_name.insert(
                    load_name,
                    TemplateLibraryReference::Unresolved {
                        known_candidate: None,
                    },
                );
                continue;
            }
        };
        let index = libraries.insert_library(library);
        result
            .loadable_by_name
            .insert(load_name, TemplateLibraryReference::Resolved(index));
    }

    let builtins = DEFAULT_TEMPLATE_BUILTINS
        .iter()
        .map(|name| PythonModuleName::parse(name).expect("default builtin is a valid module name"))
        .chain(configured_builtins.iter().cloned());
    for module_name in builtins {
        let library = match library_from_module_name(db, project, module_name) {
            ConfiguredLibraryModule::Source {
                key,
                module,
                symbols,
                recovered,
            } => {
                if recovered {
                    libraries.issues.push(TemplateLibraryIssue::BuiltinSource);
                }
                TemplateLibrary::builtin(key, module, symbols)
            }
            ConfiguredLibraryModule::SourceLess { key, module } => {
                TemplateLibrary::source_less_builtin(key, module)
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

fn insert_installed_libraries(
    libraries: &mut TemplateLibraries,
    installed_modules: &mut BTreeSet<PythonModuleName>,
    configuration: &mut BTreeMap<LibraryName, usize>,
    discovered: Vec<DiscoveredLibrary>,
) {
    for (load_name, key, module, symbols) in discovered {
        installed_modules.insert(module.name().clone());
        let index = libraries.insert_library(TemplateLibrary::installed(
            key,
            load_name.clone(),
            module,
            symbols,
        ));
        configuration.insert(load_name, index);
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
        let key = TemplateLibraryKey::new(
            db,
            Some(candidate.module.file()),
            candidate.module.name().clone(),
        );
        let facts = template_library_definition_facts(db, key);
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
                key,
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
            key: TemplateLibraryKey::new(db, None, module_name.clone()),
            module: module_name,
        };
    };
    let key = TemplateLibraryKey::new(db, Some(module.file()), module.name().clone());
    let facts = template_library_definition_facts(db, key);
    if facts.is_library() {
        ConfiguredLibraryModule::Source {
            key,
            module,
            symbols: facts.symbols().cloned().collect(),
            recovered: facts.is_recovered(),
        }
    } else {
        ConfiguredLibraryModule::NotLibrary
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_lookup_ignores_unselected_configuration_omissions() {
        let configurations = TemplateConfigurations::for_testing(&[1], false);
        let configuration = &configurations.configurations()[0];
        let backend = configuration.backends()[0].id();
        let libraries = TemplateLibraries {
            configurations: TemplateLibraryConfigurations::WithOmissions {
                known: vec![TemplateLibraryConfiguration {
                    id: configuration.id(),
                    slots: vec![TemplateLibrarySlot::Backend(
                        backend,
                        TemplateBackendLibraries::default(),
                    )],
                    guidance: KnowledgeState::Complete,
                }],
                omissions: vec![TemplateConfigurationOmission::Settings],
            },
            ..TemplateLibraries::default()
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
            libraries.environment_symbol_lookup_in_scope(
                &scoped,
                "missing",
                TemplateSymbolKind::Tag,
            ),
            EnvironmentSymbolLookup::Absent
        );
        assert_eq!(
            libraries.template_symbol_lookup_in_scope(&scoped, "missing", TemplateSymbolKind::Tag,),
            TemplateSymbolLookup::Absent
        );
        assert_eq!(
            libraries.missing_library_lookup_in_scope(&scoped, &name),
            MissingLibraryLookup::Absent
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
        let chains = libraries.contextual_library_chains_in_scope(&scoped, &[]);
        assert_eq!(chains.len(), 1);
        assert!(chains[0].steps().is_empty());
        assert_eq!(
            libraries.loadable_library_in_scope(&TemplateBackendScope::project_inventory(), &name,),
            LoadableLibraryLookup::Inconclusive(Vec::new())
        );
    }
}

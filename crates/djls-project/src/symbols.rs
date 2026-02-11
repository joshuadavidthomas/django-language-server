use std::collections::BTreeMap;
use std::collections::HashMap;

use camino::Utf8PathBuf;
use rustc_hash::FxHashSet;
use serde::Deserialize;
use serde::Serialize;

use crate::inspector::InspectorRequest;
use crate::names::LibraryName;
use crate::names::PyModuleName;
use crate::names::TemplateSymbolName;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TemplateSymbolKind {
    Tag,
    Filter,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SymbolDefinition {
    Exact { file: Utf8PathBuf },
    Module(PyModuleName),
    LibraryFile(Utf8PathBuf),
    Unknown,
}

impl SymbolDefinition {
    fn rank(&self) -> u8 {
        match self {
            Self::Exact { .. } => 3,
            Self::Module(_) => 2,
            Self::LibraryFile(_) => 1,
            Self::Unknown => 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateSymbol {
    pub kind: TemplateSymbolKind,
    pub name: TemplateSymbolName,
    pub definition: SymbolDefinition,
    #[serde(default)]
    pub doc: Option<String>,
}

impl TemplateSymbol {
    #[must_use]
    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    #[must_use]
    pub fn doc(&self) -> Option<&str> {
        self.doc.as_deref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryOrigin {
    pub app: PyModuleName,
    pub module: PyModuleName,
    pub path: Utf8PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LibraryStatus {
    Discovered(LibraryOrigin),
    Active {
        module: PyModuleName,
        origin: Option<LibraryOrigin>,
    },
    Builtin {
        module: PyModuleName,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateLibrary {
    pub name: LibraryName,
    pub status: LibraryStatus,
    #[serde(default)]
    pub symbols: Vec<TemplateSymbol>,
}

impl TemplateLibrary {
    #[must_use]
    pub fn new_discovered(name: LibraryName, origin: LibraryOrigin) -> Self {
        Self {
            name,
            status: LibraryStatus::Discovered(origin),
            symbols: Vec::new(),
        }
    }

    #[must_use]
    pub fn new_active(
        name: LibraryName,
        module: PyModuleName,
        origin: Option<LibraryOrigin>,
    ) -> Self {
        Self {
            name,
            status: LibraryStatus::Active { module, origin },
            symbols: Vec::new(),
        }
    }

    #[must_use]
    pub fn new_builtin(name: LibraryName, module: PyModuleName) -> Self {
        Self {
            name,
            status: LibraryStatus::Builtin { module },
            symbols: Vec::new(),
        }
    }

    #[must_use]
    pub fn module(&self) -> &PyModuleName {
        match &self.status {
            LibraryStatus::Discovered(origin) => &origin.module,
            LibraryStatus::Active { module, .. } | LibraryStatus::Builtin { module } => module,
        }
    }

    #[must_use]
    pub fn origin(&self) -> Option<&LibraryOrigin> {
        match &self.status {
            LibraryStatus::Discovered(origin) => Some(origin),
            LibraryStatus::Active { origin, .. } => origin.as_ref(),
            LibraryStatus::Builtin { .. } => None,
        }
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        matches!(
            self.status,
            LibraryStatus::Active { .. } | LibraryStatus::Builtin { .. }
        )
    }

    pub fn merge_symbol(&mut self, new_symbol: TemplateSymbol) {
        if let Some(existing) = self
            .symbols
            .iter_mut()
            .find(|sym| sym.kind == new_symbol.kind && sym.name == new_symbol.name)
        {
            if existing.doc.is_none() {
                existing.doc = new_symbol.doc;
            }

            if new_symbol.definition.rank() > existing.definition.rank() {
                existing.definition = new_symbol.definition;
            }

            return;
        }

        self.symbols.push(new_symbol);
        self.symbols
            .sort_by(|a, b| a.kind.cmp(&b.kind).then(a.name.cmp(&b.name)));
        self.symbols
            .dedup_by(|a, b| a.kind == b.kind && a.name == b.name);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Knowledge {
    Known,
    Unknown,
}

#[derive(Serialize)]
pub struct TemplateLibrariesRequest;

#[derive(Deserialize)]
pub struct TemplateLibrariesResponse {
    pub symbols: Vec<InspectorLibrarySymbol>,
    pub libraries: BTreeMap<String, String>,
    pub builtins: Vec<String>,
}

impl InspectorRequest for TemplateLibrariesRequest {
    const NAME: &'static str = "template_libraries";
    type Response = TemplateLibrariesResponse;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstalledSymbolOrigin {
    Builtin { module: PyModuleName },
    Loadable { load_name: LibraryName },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledSymbolCandidate {
    pub symbol: TemplateSymbol,
    pub origin: InstalledSymbolOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredSymbolCandidate {
    pub app_module: PyModuleName,
    pub library_name: LibraryName,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateLibraries {
    pub inspector_knowledge: Knowledge,
    pub discovery_knowledge: Knowledge,
    pub loadable: BTreeMap<LibraryName, Vec<TemplateLibrary>>,
    pub builtins: BTreeMap<PyModuleName, TemplateLibrary>,
}

impl Default for TemplateLibraries {
    fn default() -> Self {
        Self {
            inspector_knowledge: Knowledge::Unknown,
            discovery_knowledge: Knowledge::Unknown,
            loadable: BTreeMap::new(),
            builtins: BTreeMap::new(),
        }
    }
}

impl TemplateLibraries {
    #[must_use]
    pub fn registration_modules(&self) -> FxHashSet<PyModuleName> {
        if self.inspector_knowledge != Knowledge::Known {
            return FxHashSet::default();
        }

        let mut modules = FxHashSet::default();

        for (_name, library) in self.enabled_loadable_libraries() {
            modules.insert(library.module().clone());
        }

        for module in self.builtin_modules() {
            modules.insert(module.clone());
        }

        modules
    }

    pub fn builtin_modules(&self) -> impl Iterator<Item = &PyModuleName> + '_ {
        self.builtins.keys()
    }

    pub fn builtin_libraries(&self) -> impl Iterator<Item = &TemplateLibrary> + '_ {
        self.builtins.values()
    }

    pub fn builtin_libraries_by_module(
        &self,
    ) -> impl Iterator<Item = (&PyModuleName, &TemplateLibrary)> + '_ {
        self.builtins.iter()
    }

    pub fn loadable_library_names(&self) -> impl Iterator<Item = &LibraryName> + '_ {
        self.loadable.keys()
    }

    #[must_use]
    pub fn completion_library_names(&self) -> Vec<LibraryName> {
        let mut names: Vec<LibraryName> = self
            .loadable_library_names()
            .filter(|name| self.is_enabled_library(name) || self.has_discovered_library(name))
            .cloned()
            .collect();

        names.sort();
        names.dedup();
        names
    }

    pub fn loadable_libraries(
        &self,
    ) -> impl Iterator<Item = (&LibraryName, &TemplateLibrary)> + '_ {
        self.loadable
            .iter()
            .flat_map(|(name, libraries)| libraries.iter().map(move |library| (name, library)))
    }

    pub fn enabled_loadable_libraries(
        &self,
    ) -> impl Iterator<Item = (&LibraryName, &TemplateLibrary)> + '_ {
        self.loadable_libraries()
            .filter(|(_name, library)| library.is_active())
    }

    pub fn discovered_loadable_libraries(
        &self,
    ) -> impl Iterator<Item = (&LibraryName, &TemplateLibrary)> + '_ {
        self.loadable_libraries()
            .filter(|(_name, library)| library.origin().is_some())
    }

    #[must_use]
    pub fn installed_symbol_candidates(
        &self,
        kind: TemplateSymbolKind,
    ) -> Vec<InstalledSymbolCandidate> {
        let mut candidates = Vec::new();

        for (module, library) in self.builtin_libraries_by_module() {
            for symbol in &library.symbols {
                if symbol.kind != kind {
                    continue;
                }

                candidates.push(InstalledSymbolCandidate {
                    symbol: symbol.clone(),
                    origin: InstalledSymbolOrigin::Builtin {
                        module: module.clone(),
                    },
                });
            }
        }

        for (name, library) in self.enabled_loadable_libraries() {
            for symbol in &library.symbols {
                if symbol.kind != kind {
                    continue;
                }

                candidates.push(InstalledSymbolCandidate {
                    symbol: symbol.clone(),
                    origin: InstalledSymbolOrigin::Loadable {
                        load_name: name.clone(),
                    },
                });
            }
        }

        candidates
    }

    #[must_use]
    pub fn best_loadable_library(&self, name: &LibraryName) -> Option<&TemplateLibrary> {
        let libraries = self.loadable.get(name)?;

        libraries
            .iter()
            .find(|library| library.is_active())
            .or_else(|| libraries.iter().find(|library| library.origin().is_some()))
            .or_else(|| libraries.first())
    }

    #[must_use]
    pub fn best_loadable_library_str(&self, name: &str) -> Option<&TemplateLibrary> {
        let name = LibraryName::parse(name).ok()?;
        self.best_loadable_library(&name)
    }

    #[must_use]
    pub fn loadable_library_module(&self, name: &LibraryName) -> Option<&PyModuleName> {
        self.best_loadable_library(name)
            .map(TemplateLibrary::module)
    }

    #[must_use]
    pub fn loadable_library_module_str(&self, name: &str) -> Option<&PyModuleName> {
        let name = LibraryName::parse(name).ok()?;
        self.loadable_library_module(&name)
    }

    #[must_use]
    pub fn is_enabled_library(&self, name: &LibraryName) -> bool {
        self.loadable
            .get(name)
            .is_some_and(|libraries| libraries.iter().any(TemplateLibrary::is_active))
    }

    #[must_use]
    pub fn is_enabled_library_str(&self, name: &str) -> bool {
        LibraryName::parse(name).is_ok_and(|name| self.is_enabled_library(&name))
    }

    #[must_use]
    pub fn has_discovered_library(&self, name: &LibraryName) -> bool {
        if self.discovery_knowledge != Knowledge::Known {
            return false;
        }

        self.loadable
            .get(name)
            .is_some_and(|libraries| libraries.iter().any(|lib| lib.origin().is_some()))
    }

    #[must_use]
    pub fn discovered_app_modules_for_library(&self, name: &LibraryName) -> Vec<PyModuleName> {
        if self.discovery_knowledge != Knowledge::Known {
            return Vec::new();
        }

        self.loadable
            .get(name)
            .into_iter()
            .flatten()
            .filter_map(|library| library.origin().map(|origin| origin.app.clone()))
            .collect()
    }

    #[must_use]
    pub fn discovered_app_modules_for_library_str(&self, name: &str) -> Vec<String> {
        let Ok(name) = LibraryName::parse(name) else {
            return Vec::new();
        };

        self.discovered_app_modules_for_library(&name)
            .into_iter()
            .map(|m| m.as_str().to_string())
            .collect()
    }

    #[must_use]
    pub fn discovered_symbol_candidates_by_name(
        &self,
        kind: TemplateSymbolKind,
    ) -> Option<HashMap<TemplateSymbolName, Vec<DiscoveredSymbolCandidate>>> {
        if self.discovery_knowledge != Knowledge::Known {
            return None;
        }

        let mut map: HashMap<TemplateSymbolName, Vec<DiscoveredSymbolCandidate>> = HashMap::new();

        for (library_name, library) in self.discovered_loadable_libraries() {
            let Some(origin) = library.origin() else {
                continue;
            };

            for symbol in &library.symbols {
                if symbol.kind != kind {
                    continue;
                }

                map.entry(symbol.name.clone())
                    .or_default()
                    .push(DiscoveredSymbolCandidate {
                        app_module: origin.app.clone(),
                        library_name: library_name.clone(),
                    });
            }
        }

        Some(map)
    }

    #[must_use]
    pub fn discovered_symbol_names(&self, kind: TemplateSymbolKind) -> Vec<TemplateSymbolName> {
        let Some(map) = self.discovered_symbol_candidates_by_name(kind) else {
            return Vec::new();
        };

        let mut names: Vec<TemplateSymbolName> = map.keys().cloned().collect();
        names.sort();
        names.dedup();
        names
    }

    #[must_use]
    pub fn apply_discovery<I>(mut self, discovered: I) -> Self
    where
        I: IntoIterator<Item = TemplateLibrary>,
    {
        self.discovery_knowledge = Knowledge::Known;

        for mut lib in discovered {
            let Some(origin) = lib.origin().cloned() else {
                continue;
            };

            let entry = self.loadable.entry(lib.name.clone()).or_default();

            if let Some(existing) = entry
                .iter_mut()
                .find(|existing| existing.module() == &origin.module)
            {
                if let LibraryStatus::Active {
                    origin: existing_origin,
                    ..
                } = &mut existing.status
                {
                    if existing_origin.is_none() {
                        *existing_origin = Some(origin.clone());
                    }
                }

                for symbol in lib.symbols.drain(..) {
                    existing.merge_symbol(symbol);
                }
            } else {
                entry.push(lib);
            }
        }

        self
    }

    #[must_use]
    pub fn apply_inspector(mut self, response: Option<TemplateLibrariesResponse>) -> Self {
        let Some(response) = response else {
            self.inspector_knowledge = Knowledge::Unknown;
            self.builtins.clear();
            return self;
        };

        self.inspector_knowledge = Knowledge::Known;

        let mut enabled: BTreeMap<LibraryName, PyModuleName> = BTreeMap::new();
        for (name, module) in response.libraries {
            let Ok(name) = LibraryName::parse(&name) else {
                continue;
            };
            let Ok(module) = PyModuleName::parse(&module) else {
                continue;
            };
            enabled.insert(name, module);
        }

        for (name, module) in &enabled {
            let entry = self.loadable.entry(name.clone()).or_default();

            if let Some(existing) = entry
                .iter_mut()
                .find(|existing| existing.module() == module)
            {
                let origin = existing.origin().cloned();
                existing.status = LibraryStatus::Active {
                    module: module.clone(),
                    origin,
                };
            } else {
                entry.push(TemplateLibrary::new_active(
                    name.clone(),
                    module.clone(),
                    None,
                ));
            }
        }

        for (name, libraries) in &mut self.loadable {
            let enabled_module = enabled.get(name);

            for library in libraries.iter_mut() {
                let current_module = library.module().clone();
                let Some(active_module) = enabled_module else {
                    if let Some(origin) = library.origin().cloned() {
                        library.status = LibraryStatus::Discovered(origin);
                    }
                    continue;
                };

                if &current_module != active_module {
                    if let Some(origin) = library.origin().cloned() {
                        library.status = LibraryStatus::Discovered(origin);
                    }
                }
            }

            libraries.retain(|library| match &library.status {
                LibraryStatus::Active { module, origin } => {
                    enabled_module == Some(module) || origin.is_some()
                }
                _ => true,
            });
        }

        self.builtins.clear();
        for builtin_module in response.builtins {
            let Ok(module) = PyModuleName::parse(&builtin_module) else {
                continue;
            };
            let Ok(name) =
                LibraryName::parse(module.as_str().split('.').next_back().unwrap_or("unknown"))
            else {
                continue;
            };

            self.builtins
                .entry(module.clone())
                .or_insert_with(|| TemplateLibrary::new_builtin(name, module));
        }

        for symbol in response.symbols {
            self.apply_inspector_symbol(&enabled, symbol);
        }

        self
    }

    fn apply_inspector_symbol(
        &mut self,
        enabled: &BTreeMap<LibraryName, PyModuleName>,
        wire: InspectorLibrarySymbol,
    ) {
        let Some(kind) = wire.kind else {
            return;
        };

        let Ok(name) = TemplateSymbolName::parse(&wire.name) else {
            return;
        };

        let definition = PyModuleName::parse(&wire.module)
            .map(SymbolDefinition::Module)
            .unwrap_or(SymbolDefinition::Unknown);

        let symbol = TemplateSymbol {
            kind,
            name,
            definition,
            doc: wire.doc,
        };

        match wire.load_name {
            None => {
                let Ok(module) = PyModuleName::parse(&wire.library_module) else {
                    return;
                };

                if let Some(library) = self.builtins.get_mut(&module) {
                    library.merge_symbol(symbol);
                }
            }
            Some(load_name) => {
                let Ok(library_name) = LibraryName::parse(&load_name) else {
                    return;
                };

                let module = enabled
                    .get(&library_name)
                    .cloned()
                    .or_else(|| PyModuleName::parse(&wire.library_module).ok());

                let Some(module) = module else {
                    return;
                };

                if let Some(libraries) = self.loadable.get_mut(&library_name) {
                    if let Some(library) = libraries.iter_mut().find(|l| l.module() == &module) {
                        library.merge_symbol(symbol);
                    }
                }
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct InspectorLibrarySymbol {
    #[serde(default)]
    pub kind: Option<TemplateSymbolKind>,
    pub name: String,
    #[serde(default)]
    pub load_name: Option<String>,
    pub library_module: String,
    pub module: String,
    #[serde(default)]
    pub doc: Option<String>,
}

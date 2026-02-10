use std::collections::BTreeMap;
use std::collections::HashMap;

use camino::Utf8PathBuf;
use rustc_hash::FxHashSet;
use serde::Deserialize;
use serde::Serialize;

use crate::inspector::InspectorRequest;
use crate::scanned_libraries::ScannedTemplateLibraries;
use crate::scanned_libraries::ScannedTemplateLibrary;
use crate::template_names::LibraryName;
use crate::template_names::PyModuleName;
use crate::template_names::TemplateSymbolName;

#[derive(Serialize)]
pub struct TemplateLibrariesRequest;

#[derive(Deserialize)]
pub struct TemplateLibrariesResponse {
    pub symbols: Vec<InspectorTemplateLibrarySymbolWire>,
    pub libraries: BTreeMap<String, String>,
    pub builtins: Vec<String>,
}

impl InspectorRequest for TemplateLibrariesRequest {
    const NAME: &'static str = "template_libraries";
    type Response = TemplateLibrariesResponse;
}

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TemplateLibraryId {
    Loadable {
        name: LibraryName,
        module: PyModuleName,
    },
    Builtin {
        module: PyModuleName,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Knowledge {
    Known,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum LibraryEnablement {
    Enabled,
    NotEnabled,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LibraryLocation {
    Scanned {
        app_module: PyModuleName,
        source_path: Utf8PathBuf,
    },
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateLibrary {
    pub id: TemplateLibraryId,
    pub enablement: LibraryEnablement,
    pub location: LibraryLocation,
    #[serde(default)]
    pub symbols: Vec<TemplateSymbol>,
}

impl TemplateLibrary {
    fn new_loadable(name: LibraryName, module: PyModuleName) -> Self {
        Self {
            id: TemplateLibraryId::Loadable { name, module },
            enablement: LibraryEnablement::Unknown,
            location: LibraryLocation::Unknown,
            symbols: Vec::new(),
        }
    }

    fn new_builtin(module: PyModuleName) -> Self {
        Self {
            id: TemplateLibraryId::Builtin { module },
            enablement: LibraryEnablement::Enabled,
            location: LibraryLocation::Unknown,
            symbols: Vec::new(),
        }
    }

    fn merge_symbol(&mut self, new_symbol: TemplateSymbol) {
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateLibraries {
    pub inspector_knowledge: Knowledge,
    pub scan_knowledge: Knowledge,

    /// Loadable libraries grouped by `{% load name %}`.
    pub loadable: BTreeMap<LibraryName, Vec<TemplateLibrary>>,

    /// Builtin libraries keyed by their module.
    pub builtins: BTreeMap<PyModuleName, TemplateLibrary>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScannedSymbolCandidate {
    pub app_module: PyModuleName,
    pub library_name: LibraryName,
}

impl Default for TemplateLibraries {
    fn default() -> Self {
        Self {
            inspector_knowledge: Knowledge::Unknown,
            scan_knowledge: Knowledge::Unknown,
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

        for libraries in self.loadable.values() {
            for library in libraries {
                if library.enablement != LibraryEnablement::Enabled {
                    continue;
                }

                if let TemplateLibraryId::Loadable { module, .. } = &library.id {
                    modules.insert(module.clone());
                }
            }
        }

        for module in self.builtins.keys() {
            modules.insert(module.clone());
        }

        modules
    }

    #[must_use]
    pub fn is_enabled_library(&self, name: &LibraryName) -> bool {
        self.loadable.get(name).is_some_and(|libraries| {
            libraries
                .iter()
                .any(|library| library.enablement == LibraryEnablement::Enabled)
        })
    }

    #[must_use]
    pub fn is_enabled_library_str(&self, name: &str) -> bool {
        LibraryName::new(name).is_some_and(|name| self.is_enabled_library(&name))
    }

    #[must_use]
    pub fn has_scanned_library(&self, name: &LibraryName) -> bool {
        if self.scan_knowledge != Knowledge::Known {
            return false;
        }

        self.loadable.get(name).is_some_and(|libraries| {
            libraries
                .iter()
                .any(|library| matches!(library.location, LibraryLocation::Scanned { .. }))
        })
    }

    #[must_use]
    pub fn scanned_app_modules_for_library(&self, name: &LibraryName) -> Vec<PyModuleName> {
        if self.scan_knowledge != Knowledge::Known {
            return Vec::new();
        }

        self.loadable
            .get(name)
            .into_iter()
            .flatten()
            .filter_map(|library| match &library.location {
                LibraryLocation::Scanned { app_module, .. } => Some(app_module.clone()),
                LibraryLocation::Unknown => None,
            })
            .collect()
    }

    #[must_use]
    pub fn scanned_app_modules_for_library_str(&self, name: &str) -> Vec<String> {
        let Some(name) = LibraryName::new(name) else {
            return Vec::new();
        };

        self.scanned_app_modules_for_library(&name)
            .into_iter()
            .map(|m| m.as_str().to_string())
            .collect()
    }

    #[must_use]
    pub fn scanned_symbol_candidates_by_name(
        &self,
        kind: TemplateSymbolKind,
    ) -> Option<HashMap<TemplateSymbolName, Vec<ScannedSymbolCandidate>>> {
        if self.scan_knowledge != Knowledge::Known {
            return None;
        }

        let mut map: HashMap<TemplateSymbolName, Vec<ScannedSymbolCandidate>> = HashMap::new();

        for (library_name, libraries) in &self.loadable {
            for library in libraries {
                let LibraryLocation::Scanned { app_module, .. } = &library.location else {
                    continue;
                };

                for symbol in &library.symbols {
                    if symbol.kind != kind {
                        continue;
                    }

                    map.entry(symbol.name.clone())
                        .or_default()
                        .push(ScannedSymbolCandidate {
                            app_module: app_module.clone(),
                            library_name: library_name.clone(),
                        });
                }
            }
        }

        Some(map)
    }

    #[must_use]
    pub fn scanned_symbol_names(&self, kind: TemplateSymbolKind) -> Vec<TemplateSymbolName> {
        let Some(map) = self.scanned_symbol_candidates_by_name(kind) else {
            return Vec::new();
        };

        let mut names: Vec<TemplateSymbolName> = map.keys().cloned().collect();
        names.sort();
        names.dedup();
        names
    }

    #[must_use]
    pub fn apply_scan(mut self, scanned: &ScannedTemplateLibraries) -> Self {
        self.scan_knowledge = Knowledge::Known;

        for libraries in scanned.libraries().values() {
            for library in libraries {
                self.apply_scanned_library(library);
            }
        }

        self
    }

    fn apply_scanned_library(&mut self, library: &ScannedTemplateLibrary) {
        let entry = self.loadable.entry(library.name.clone()).or_default();

        let module = library.module.clone();

        let idx = entry.iter().position(|existing| match &existing.id {
            TemplateLibraryId::Loadable {
                name: _,
                module: existing_module,
            } => *existing_module == module,
            TemplateLibraryId::Builtin { .. } => false,
        });

        let lib = if let Some(idx) = idx {
            &mut entry[idx]
        } else {
            entry.push(TemplateLibrary::new_loadable(
                library.name.clone(),
                library.module.clone(),
            ));
            let last = entry.len() - 1;
            &mut entry[last]
        };

        lib.location = LibraryLocation::Scanned {
            app_module: library.app_module.clone(),
            source_path: library.source_path.clone(),
        };

        // If the inspector is known, then any scanned library not marked Enabled is known to be
        // not enabled for this project.
        if self.inspector_knowledge == Knowledge::Known
            && lib.enablement != LibraryEnablement::Enabled
        {
            lib.enablement = LibraryEnablement::NotEnabled;
        }

        for symbol in &library.symbols {
            let template_symbol = TemplateSymbol {
                kind: symbol.kind,
                name: symbol.name.clone(),
                definition: SymbolDefinition::LibraryFile(library.source_path.clone()),
                doc: None,
            };
            lib.merge_symbol(template_symbol);
        }
    }

    #[must_use]
    pub fn apply_inspector(mut self, response: Option<TemplateLibrariesResponse>) -> Self {
        let Some(response) = response else {
            self.inspector_knowledge = Knowledge::Unknown;
            self.builtins.clear();
            for libraries in self.loadable.values_mut() {
                for library in libraries {
                    library.enablement = LibraryEnablement::Unknown;
                }
            }
            return self;
        };

        self.inspector_knowledge = Knowledge::Known;

        let mut enabled: BTreeMap<LibraryName, PyModuleName> = BTreeMap::new();
        for (name, module) in response.libraries {
            let Some(name) = LibraryName::new(&name) else {
                continue;
            };
            let Some(module) = PyModuleName::new(&module) else {
                continue;
            };
            enabled.insert(name, module);
        }

        // Ensure enabled libraries exist and mark them enabled.
        for (name, module) in &enabled {
            let entry = self.loadable.entry(name.clone()).or_default();

            let idx = entry.iter().position(|existing| match &existing.id {
                TemplateLibraryId::Loadable {
                    name: _,
                    module: existing_module,
                } => existing_module == module,
                TemplateLibraryId::Builtin { .. } => false,
            });

            let lib = if let Some(idx) = idx {
                &mut entry[idx]
            } else {
                entry.push(TemplateLibrary::new_loadable(name.clone(), module.clone()));
                let last = entry.len() - 1;
                &mut entry[last]
            };

            lib.enablement = LibraryEnablement::Enabled;
        }

        // Mark all other loadable libraries as not enabled.
        for (name, libraries) in &mut self.loadable {
            let enabled_module = enabled.get(name);

            for library in libraries {
                let module = match &library.id {
                    TemplateLibraryId::Loadable { module, .. } => module,
                    TemplateLibraryId::Builtin { .. } => continue,
                };

                if Some(module) == enabled_module {
                    library.enablement = LibraryEnablement::Enabled;
                } else {
                    library.enablement = LibraryEnablement::NotEnabled;
                }
            }
        }

        // Builtins are authoritative from inspector.
        self.builtins.clear();
        for builtin_module in response.builtins {
            let Some(module) = PyModuleName::new(&builtin_module) else {
                continue;
            };
            self.builtins
                .entry(module.clone())
                .or_insert_with(|| TemplateLibrary::new_builtin(module));
        }

        for symbol in response.symbols {
            self.apply_inspector_symbol(&enabled, symbol);
        }

        self
    }

    fn apply_inspector_symbol(
        &mut self,
        enabled: &BTreeMap<LibraryName, PyModuleName>,
        wire: InspectorTemplateLibrarySymbolWire,
    ) {
        let Some(kind) = wire.kind else {
            return;
        };

        let Some(name) = TemplateSymbolName::new(&wire.name) else {
            return;
        };

        let definition_module = PyModuleName::new(&wire.module);

        let definition = match definition_module {
            Some(module) => SymbolDefinition::Module(module),
            None => SymbolDefinition::Unknown,
        };

        let symbol = TemplateSymbol {
            kind,
            name,
            definition,
            doc: wire.doc,
        };

        match wire.load_name {
            None => {
                let Some(module) = PyModuleName::new(&wire.library_module) else {
                    return;
                };

                let library = self
                    .builtins
                    .entry(module.clone())
                    .or_insert_with(|| TemplateLibrary::new_builtin(module));

                library.merge_symbol(symbol);
            }
            Some(load_name) => {
                let Some(library_name) = LibraryName::new(&load_name) else {
                    return;
                };

                let module = enabled
                    .get(&library_name)
                    .cloned()
                    .or_else(|| PyModuleName::new(&wire.library_module));

                let Some(module) = module else {
                    return;
                };

                let entry = self.loadable.entry(library_name.clone()).or_default();

                let idx = entry.iter().position(|existing| match &existing.id {
                    TemplateLibraryId::Loadable {
                        name: _,
                        module: existing_module,
                    } => existing_module == &module,
                    TemplateLibraryId::Builtin { .. } => false,
                });

                let library = if let Some(idx) = idx {
                    &mut entry[idx]
                } else {
                    entry.push(TemplateLibrary::new_loadable(
                        library_name.clone(),
                        module.clone(),
                    ));
                    let last = entry.len() - 1;
                    &mut entry[last]
                };

                library.enablement = LibraryEnablement::Enabled;
                library.merge_symbol(symbol);
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct InspectorTemplateLibrarySymbolWire {
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

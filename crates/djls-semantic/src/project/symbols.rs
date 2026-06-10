use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use serde::Deserialize;
use serde::Serialize;

use crate::project::names::LibraryName;
use crate::project::names::PyModuleName;
use crate::project::names::TemplateSymbolName;

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
            LibraryStatus::Active { module, .. } | LibraryStatus::Builtin { module } => module,
        }
    }

    #[must_use]
    pub fn origin(&self) -> Option<&LibraryOrigin> {
        match &self.status {
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateLibrarySnapshot {
    pub symbols: Vec<TemplateSymbolSnapshot>,
    pub libraries: BTreeMap<String, String>,
    pub builtins: Vec<String>,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateLibraries {
    pub active_knowledge: Knowledge,
    pub loadable: BTreeMap<LibraryName, Vec<TemplateLibrary>>,
    pub builtins: BTreeMap<PyModuleName, TemplateLibrary>,
    pub builtin_order: Vec<PyModuleName>,
}

impl Default for TemplateLibraries {
    fn default() -> Self {
        Self {
            active_knowledge: Knowledge::Unknown,
            loadable: BTreeMap::new(),
            builtins: BTreeMap::new(),
            builtin_order: Vec::new(),
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
    pub fn registration_modules(&self) -> Vec<PyModuleName> {
        if self.active_knowledge != Knowledge::Known {
            return Vec::new();
        }

        let mut modules = Vec::new();

        for (_name, library) in self.enabled_loadable_libraries() {
            push_unique_module(&mut modules, library.module().clone());
        }

        for module in self.builtin_modules() {
            push_unique_module(&mut modules, module.clone());
        }

        modules
    }

    pub fn builtin_modules(&self) -> impl Iterator<Item = &PyModuleName> + '_ {
        self.builtin_order
            .iter()
            .filter(|module| self.builtins.contains_key(*module))
    }

    pub fn builtin_libraries(&self) -> impl Iterator<Item = &TemplateLibrary> + '_ {
        self.builtin_modules()
            .filter_map(|module| self.builtins.get(module))
    }

    pub fn builtin_libraries_by_module(
        &self,
    ) -> impl Iterator<Item = (&PyModuleName, &TemplateLibrary)> + '_ {
        self.builtin_modules()
            .filter_map(|module| self.builtins.get(module).map(|library| (module, library)))
    }

    pub fn loadable_library_names(&self) -> impl Iterator<Item = &LibraryName> + '_ {
        self.loadable.keys()
    }

    #[must_use]
    pub fn completion_library_names(&self) -> Vec<LibraryName> {
        let mut names: Vec<LibraryName> = self
            .loadable_library_names()
            .filter(|name| self.is_enabled_library(name))
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

    #[must_use]
    pub fn installed_symbol_candidates(
        &self,
        kind: TemplateSymbolKind,
    ) -> Vec<InstalledSymbolCandidate> {
        let mut candidates = Vec::new();
        let mut builtin_candidates = BTreeMap::new();

        for (module, library) in self.builtin_libraries_by_module() {
            for symbol in &library.symbols {
                if symbol.kind != kind {
                    continue;
                }

                builtin_candidates.insert(
                    symbol.name.clone(),
                    InstalledSymbolCandidate {
                        symbol: symbol.clone(),
                        origin: InstalledSymbolOrigin::Builtin {
                            module: module.clone(),
                        },
                    },
                );
            }
        }
        candidates.extend(builtin_candidates.into_values());

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
    pub fn apply_active_snapshot(mut self, response: Option<TemplateLibrarySnapshot>) -> Self {
        let Some(response) = response else {
            self.active_knowledge = Knowledge::Unknown;
            self.builtins.clear();
            self.builtin_order.clear();
            return self;
        };

        self.active_knowledge = Knowledge::Known;

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

            libraries.retain(|library| match &library.status {
                LibraryStatus::Active { module, .. } => enabled_module == Some(module),
                LibraryStatus::Builtin { .. } => true,
            });

            for library in libraries.iter_mut().filter(|library| library.is_active()) {
                library.symbols.clear();
            }
        }

        self.builtins.clear();
        self.builtin_order.clear();
        for builtin_module in response.builtins {
            let Ok(module) = PyModuleName::parse(&builtin_module) else {
                continue;
            };
            let Ok(name) =
                LibraryName::parse(module.as_str().split('.').next_back().unwrap_or("unknown"))
            else {
                continue;
            };

            push_unique_module(&mut self.builtin_order, module.clone());
            self.builtins
                .entry(module.clone())
                .or_insert_with(|| TemplateLibrary::new_builtin(name, module));
        }

        for symbol in response.symbols {
            self.apply_active_snapshot_symbol(&enabled, symbol);
        }

        self
    }

    fn apply_active_snapshot_symbol(
        &mut self,
        enabled: &BTreeMap<LibraryName, PyModuleName>,
        snapshot: TemplateSymbolSnapshot,
    ) {
        let Some(kind) = snapshot.kind else {
            return;
        };

        let Ok(name) = TemplateSymbolName::parse(&snapshot.name) else {
            return;
        };

        let definition = PyModuleName::parse(&snapshot.module)
            .map_or(SymbolDefinition::Unknown, SymbolDefinition::Module);

        let symbol = TemplateSymbol {
            kind,
            name,
            definition,
            doc: snapshot.doc,
        };

        match snapshot.load_name {
            None => {
                let Ok(module) = PyModuleName::parse(&snapshot.library_module) else {
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
                    .or_else(|| PyModuleName::parse(&snapshot.library_module).ok());

                let Some(module) = module else {
                    return;
                };

                if let Some(libraries) = self.loadable.get_mut(&library_name)
                    && let Some(library) = libraries.iter_mut().find(|l| l.module() == &module)
                {
                    library.merge_symbol(symbol);
                }
            }
        }
    }
}

fn push_unique_module(modules: &mut Vec<PyModuleName>, module: PyModuleName) {
    if !modules.contains(&module) {
        modules.push(module);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateSymbolSnapshot {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_candidates_keep_last_builtin_symbol() {
        let libraries =
            TemplateLibraries::default().apply_active_snapshot(Some(TemplateLibrarySnapshot {
                symbols: vec![
                    TemplateSymbolSnapshot {
                        kind: Some(TemplateSymbolKind::Filter),
                        name: "duplicate".to_string(),
                        load_name: None,
                        library_module: "z_first".to_string(),
                        module: "z_first".to_string(),
                        doc: Some("first".to_string()),
                    },
                    TemplateSymbolSnapshot {
                        kind: Some(TemplateSymbolKind::Filter),
                        name: "duplicate".to_string(),
                        load_name: None,
                        library_module: "a_second".to_string(),
                        module: "a_second".to_string(),
                        doc: Some("second".to_string()),
                    },
                ],
                libraries: BTreeMap::new(),
                builtins: vec!["z_first".to_string(), "a_second".to_string()],
            }));

        let candidates = libraries.installed_symbol_candidates(TemplateSymbolKind::Filter);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].symbol.doc.as_deref(), Some("second"));
        assert_eq!(
            candidates[0].origin,
            InstalledSymbolOrigin::Builtin {
                module: PyModuleName::parse("a_second").unwrap()
            }
        );
    }

    #[test]
    fn active_snapshot_replaces_loadable_symbols() {
        let first = TemplateLibrarySnapshot {
            symbols: vec![TemplateSymbolSnapshot {
                kind: Some(TemplateSymbolKind::Tag),
                name: "old_tag".to_string(),
                load_name: Some("project_tags".to_string()),
                library_module: "project.templatetags.project_tags".to_string(),
                module: "project.templatetags.project_tags".to_string(),
                doc: None,
            }],
            libraries: [(
                "project_tags".to_string(),
                "project.templatetags.project_tags".to_string(),
            )]
            .into(),
            builtins: Vec::new(),
        };
        let second = TemplateLibrarySnapshot {
            symbols: vec![TemplateSymbolSnapshot {
                kind: Some(TemplateSymbolKind::Tag),
                name: "new_tag".to_string(),
                load_name: Some("project_tags".to_string()),
                library_module: "project.templatetags.project_tags".to_string(),
                module: "project.templatetags.project_tags".to_string(),
                doc: None,
            }],
            libraries: [(
                "project_tags".to_string(),
                "project.templatetags.project_tags".to_string(),
            )]
            .into(),
            builtins: Vec::new(),
        };

        let libraries = TemplateLibraries::default()
            .apply_active_snapshot(Some(first))
            .apply_active_snapshot(Some(second));

        let names: Vec<_> = libraries
            .installed_symbol_candidates(TemplateSymbolKind::Tag)
            .into_iter()
            .map(|candidate| candidate.symbol.name().to_string())
            .collect();

        assert_eq!(names, vec!["new_tag"]);
    }

    #[test]
    fn registration_modules_keep_deterministic_precedence_order() {
        let libraries =
            TemplateLibraries::default().apply_active_snapshot(Some(TemplateLibrarySnapshot {
                symbols: Vec::new(),
                libraries: [("project_tags".to_string(), "project.tags".to_string())].into(),
                builtins: vec![
                    "z.templatetags.tags".to_string(),
                    "django.template.defaulttags".to_string(),
                    "z.templatetags.tags".to_string(),
                ],
            }));

        let modules: Vec<_> = libraries
            .registration_modules()
            .into_iter()
            .map(|module| module.as_str().to_string())
            .collect();

        assert_eq!(
            modules,
            vec![
                "project.tags",
                "z.templatetags.tags",
                "django.template.defaulttags",
            ]
        );
    }
}

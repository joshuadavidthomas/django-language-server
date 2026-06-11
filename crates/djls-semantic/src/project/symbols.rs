use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use djls_project::StaticKnowledge;
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

    pub(crate) fn merge_symbols(&mut self, symbols: impl IntoIterator<Item = TemplateSymbol>) {
        for symbol in symbols {
            self.merge_symbol(symbol);
        }
    }
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
    pub knowledge: StaticKnowledge,
    pub loadable: BTreeMap<LibraryName, Vec<TemplateLibrary>>,
    pub builtins: BTreeMap<PyModuleName, TemplateLibrary>,
    pub builtin_order: Vec<PyModuleName>,
}

impl Default for TemplateLibraries {
    fn default() -> Self {
        Self {
            knowledge: StaticKnowledge::Unknown,
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
        if self.knowledge == StaticKnowledge::Unknown {
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
}

fn push_unique_module(modules: &mut Vec<PyModuleName>, module: PyModuleName) {
    if !modules.contains(&module) {
        modules.push(module);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(name: &str) -> PyModuleName {
        PyModuleName::parse(name).unwrap()
    }

    fn library_name(name: &str) -> LibraryName {
        LibraryName::parse(name).unwrap()
    }

    fn symbol(kind: TemplateSymbolKind, name: &str, doc: Option<&str>) -> TemplateSymbol {
        TemplateSymbol {
            kind,
            name: TemplateSymbolName::parse(name).unwrap(),
            definition: SymbolDefinition::Unknown,
            doc: doc.map(str::to_string),
        }
    }

    fn builtin_library(
        name: &str,
        module_name: &str,
        symbols: Vec<TemplateSymbol>,
    ) -> TemplateLibrary {
        let mut library = TemplateLibrary::new_builtin(library_name(name), module(module_name));
        for symbol in symbols {
            library.merge_symbol(symbol);
        }
        library
    }

    fn active_library(name: &str, module_name: &str) -> TemplateLibrary {
        TemplateLibrary::new_active(library_name(name), module(module_name), None)
    }

    #[test]
    fn builtin_candidates_keep_last_builtin_symbol() {
        let mut libraries = TemplateLibraries {
            knowledge: StaticKnowledge::Known,
            ..TemplateLibraries::default()
        };
        let z_first = module("z_first");
        let a_second = module("a_second");
        libraries.builtin_order.push(z_first.clone());
        libraries.builtin_order.push(a_second.clone());
        libraries.builtins.insert(
            z_first,
            builtin_library(
                "z_first",
                "z_first",
                vec![symbol(
                    TemplateSymbolKind::Filter,
                    "duplicate",
                    Some("first"),
                )],
            ),
        );
        libraries.builtins.insert(
            a_second.clone(),
            builtin_library(
                "a_second",
                "a_second",
                vec![symbol(
                    TemplateSymbolKind::Filter,
                    "duplicate",
                    Some("second"),
                )],
            ),
        );

        let candidates = libraries.installed_symbol_candidates(TemplateSymbolKind::Filter);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].symbol.doc.as_deref(), Some("second"));
        assert_eq!(
            candidates[0].origin,
            InstalledSymbolOrigin::Builtin { module: a_second }
        );
    }

    #[test]
    fn registration_modules_keep_deterministic_precedence_order() {
        let mut libraries = TemplateLibraries {
            knowledge: StaticKnowledge::Known,
            ..TemplateLibraries::default()
        };
        libraries
            .loadable
            .entry(library_name("project_tags"))
            .or_default()
            .push(active_library("project_tags", "project.tags"));
        for module_name in [
            "z.templatetags.tags",
            "django.template.defaulttags",
            "z.templatetags.tags",
        ] {
            let module = module(module_name);
            push_unique_module(&mut libraries.builtin_order, module.clone());
            libraries
                .builtins
                .entry(module.clone())
                .or_insert_with(|| builtin_library("defaulttags", module.as_str(), Vec::new()));
        }

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

    #[test]
    fn registration_modules_keep_known_partial_modules() {
        let mut libraries = TemplateLibraries {
            knowledge: StaticKnowledge::Partial,
            ..TemplateLibraries::default()
        };
        libraries
            .loadable
            .entry(library_name("project_tags"))
            .or_default()
            .push(active_library("project_tags", "project.tags"));

        let modules: Vec<_> = libraries
            .registration_modules()
            .into_iter()
            .map(|module| module.as_str().to_string())
            .collect();

        assert_eq!(modules, vec!["project.tags"]);
    }
}

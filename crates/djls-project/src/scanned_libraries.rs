use std::collections::BTreeMap;
use std::collections::HashMap;

use camino::Utf8PathBuf;
use serde::Deserialize;
use serde::Serialize;

use crate::template_libraries::TemplateSymbolKind;
use crate::template_names::LibraryName;
use crate::template_names::PyModuleName;
use crate::template_names::TemplateSymbolName;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScannedTemplateSymbol {
    pub kind: TemplateSymbolKind,
    pub name: TemplateSymbolName,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScannedTemplateLibrary {
    pub name: LibraryName,
    pub app_module: PyModuleName,
    pub module: PyModuleName,
    pub source_path: Utf8PathBuf,
    #[serde(default)]
    pub symbols: Vec<ScannedTemplateSymbol>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScannedTemplateLibrarySymbol {
    pub kind: TemplateSymbolKind,
    pub name: TemplateSymbolName,
    pub library_name: LibraryName,
    pub app_module: PyModuleName,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScannedTemplateLibraries {
    libraries: BTreeMap<LibraryName, Vec<ScannedTemplateLibrary>>,
}

impl ScannedTemplateLibraries {
    #[must_use]
    pub fn new(libraries: BTreeMap<LibraryName, Vec<ScannedTemplateLibrary>>) -> Self {
        Self { libraries }
    }

    #[must_use]
    pub fn libraries(&self) -> &BTreeMap<LibraryName, Vec<ScannedTemplateLibrary>> {
        &self.libraries
    }

    #[must_use]
    pub fn has_library(&self, name: &LibraryName) -> bool {
        self.libraries.contains_key(name)
    }

    #[must_use]
    pub fn libraries_for_name(&self, name: &LibraryName) -> &[ScannedTemplateLibrary] {
        self.libraries
            .get(name)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.libraries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.libraries.is_empty()
    }

    #[must_use]
    pub fn symbols_by_name(
        &self,
        kind: TemplateSymbolKind,
    ) -> HashMap<TemplateSymbolName, Vec<ScannedTemplateLibrarySymbol>> {
        let mut map: HashMap<TemplateSymbolName, Vec<ScannedTemplateLibrarySymbol>> =
            HashMap::new();

        for (library_name, libraries) in &self.libraries {
            for library in libraries {
                for symbol in &library.symbols {
                    if symbol.kind != kind {
                        continue;
                    }

                    map.entry(symbol.name.clone()).or_default().push(
                        ScannedTemplateLibrarySymbol {
                            kind,
                            name: symbol.name.clone(),
                            library_name: library_name.clone(),
                            app_module: library.app_module.clone(),
                        },
                    );
                }
            }
        }

        map
    }

    #[must_use]
    pub fn tags_by_name(&self) -> HashMap<TemplateSymbolName, Vec<ScannedTemplateLibrarySymbol>> {
        self.symbols_by_name(TemplateSymbolKind::Tag)
    }

    #[must_use]
    pub fn filters_by_name(
        &self,
    ) -> HashMap<TemplateSymbolName, Vec<ScannedTemplateLibrarySymbol>> {
        self.symbols_by_name(TemplateSymbolKind::Filter)
    }
}

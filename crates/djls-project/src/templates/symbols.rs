use camino::Utf8PathBuf;
use serde::Deserialize;
use serde::Serialize;

use super::names::TemplateSymbolName;
use crate::python::PythonModulePath;

/// Whether a symbol is a template tag or a template filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TemplateSymbolKind {
    Tag,
    Filter,
}

/// Identifies a specific tag or filter registration within a module.
///
/// Keyed by both the registration module path and the symbol name to avoid
/// collisions when different libraries register identically-named symbols.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SymbolKey {
    pub registration_module: String,
    pub name: String,
    pub kind: TemplateSymbolKind,
}

impl SymbolKey {
    #[must_use]
    pub fn tag(registration_module: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            registration_module: registration_module.into(),
            name: name.into(),
            kind: TemplateSymbolKind::Tag,
        }
    }

    #[must_use]
    pub fn filter(registration_module: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            registration_module: registration_module.into(),
            name: name.into(),
            kind: TemplateSymbolKind::Filter,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SymbolDefinition {
    Exact { file: Utf8PathBuf },
    Module(PythonModulePath),
    LibraryFile(Utf8PathBuf),
    Unknown,
}

impl SymbolDefinition {
    pub(crate) fn rank(&self) -> u8 {
        match self {
            Self::Exact { .. } => 3,
            Self::Module(_) => 2,
            Self::LibraryFile(_) => 1,
            Self::Unknown => 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateSymbol {
    pub kind: TemplateSymbolKind,
    pub name: TemplateSymbolName,
    pub definition: SymbolDefinition,
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

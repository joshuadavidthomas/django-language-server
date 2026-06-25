use camino::Utf8PathBuf;

use crate::names::PyModuleName;
use crate::names::TemplateSymbolName;
use crate::specs::TemplateSymbolKind;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SymbolDefinition {
    Exact { file: Utf8PathBuf },
    Module(PyModuleName),
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

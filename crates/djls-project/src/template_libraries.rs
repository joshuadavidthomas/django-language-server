use std::collections::BTreeMap;

use rustc_hash::FxHashSet;
use serde::Deserialize;
use serde::Serialize;

use crate::discovered_libraries::DiscoveredTemplateLibraries;
use crate::inspector::InspectorRequest;

#[derive(Serialize)]
pub struct InstalledTemplateLibrariesRequest;

#[derive(Deserialize)]
pub struct InstalledTemplateLibrariesResponse {
    pub templatetags: Vec<InstalledTemplateTag>,
    pub templatefilters: Vec<InstalledTemplateFilter>,
    pub libraries: BTreeMap<String, String>,
    pub builtins: Vec<String>,
}

impl InspectorRequest for InstalledTemplateLibrariesRequest {
    // Inspector endpoint name is historical; it returns tags, filters, libraries, and builtins.
    const NAME: &'static str = "templatetags";
    type Response = InstalledTemplateLibrariesResponse;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateLibraries {
    installed: InstalledTemplateLibraries,
    discovered: DiscoveredTemplateLibraries,
}

impl Default for TemplateLibraries {
    fn default() -> Self {
        Self {
            installed: InstalledTemplateLibraries::Unknown,
            discovered: DiscoveredTemplateLibraries::default(),
        }
    }
}

impl TemplateLibraries {
    #[must_use]
    pub fn installed(&self) -> &InstalledTemplateLibraries {
        &self.installed
    }

    #[must_use]
    pub fn discovered(&self) -> &DiscoveredTemplateLibraries {
        &self.discovered
    }

    #[must_use]
    pub fn replace_installed(self, installed: InstalledTemplateLibraries) -> Self {
        Self { installed, ..self }
    }

    #[must_use]
    pub fn replace_discovered(self, discovered: DiscoveredTemplateLibraries) -> Self {
        Self { discovered, ..self }
    }

    #[must_use]
    pub fn installed_registration_modules(&self) -> FxHashSet<String> {
        match &self.installed {
            InstalledTemplateLibraries::Known(known) => known.registration_modules(),
            InstalledTemplateLibraries::Unknown => FxHashSet::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstalledTemplateLibraries {
    Known(KnownInstalledTemplateLibraries),
    Unknown,
}

impl InstalledTemplateLibraries {
    #[must_use]
    pub fn as_known(&self) -> Option<&KnownInstalledTemplateLibraries> {
        match self {
            Self::Known(known) => Some(known),
            Self::Unknown => None,
        }
    }

    #[must_use]
    pub fn from_response(response: InstalledTemplateLibrariesResponse) -> Self {
        Self::Known(KnownInstalledTemplateLibraries::new(
            response.templatetags,
            response.templatefilters,
            response.libraries,
            response.builtins,
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownInstalledTemplateLibraries {
    tags: Vec<InstalledTemplateTag>,
    #[serde(default)]
    filters: Vec<InstalledTemplateFilter>,
    /// Mapping from load name (`{% load X %}`) to module path.
    libraries: BTreeMap<String, String>,
    /// Dotted module paths for builtin libraries.
    builtins: Vec<String>,
}

impl KnownInstalledTemplateLibraries {
    #[must_use]
    pub fn new(
        mut tags: Vec<InstalledTemplateTag>,
        mut filters: Vec<InstalledTemplateFilter>,
        libraries: BTreeMap<String, String>,
        mut builtins: Vec<String>,
    ) -> Self {
        tags.sort_by(|a, b| a.name.cmp(&b.name));
        filters.sort_by(|a, b| a.name.cmp(&b.name));
        builtins.sort();

        Self {
            tags,
            filters,
            libraries,
            builtins,
        }
    }

    #[must_use]
    pub fn tags(&self) -> &[InstalledTemplateTag] {
        &self.tags
    }

    #[must_use]
    pub fn filters(&self) -> &[InstalledTemplateFilter] {
        &self.filters
    }

    #[must_use]
    pub fn libraries(&self) -> &BTreeMap<String, String> {
        &self.libraries
    }

    #[must_use]
    pub fn builtins(&self) -> &[String] {
        &self.builtins
    }

    #[must_use]
    pub fn registration_modules(&self) -> FxHashSet<String> {
        let mut modules = FxHashSet::default();

        for tag in &self.tags {
            modules.insert(tag.registration_module().to_string());
        }

        for filter in &self.filters {
            modules.insert(filter.registration_module().to_string());
        }

        modules
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledTemplateTag {
    pub(crate) name: String,
    pub(crate) provenance: InstalledSymbolProvenance,
    pub(crate) defining_module: String,
    pub(crate) doc: Option<String>,
}

impl InstalledTemplateTag {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn provenance(&self) -> &InstalledSymbolProvenance {
        &self.provenance
    }

    #[must_use]
    pub fn defining_module(&self) -> &str {
        &self.defining_module
    }

    #[must_use]
    pub fn doc(&self) -> Option<&str> {
        self.doc.as_deref()
    }

    #[must_use]
    pub fn library_load_name(&self) -> Option<&str> {
        self.provenance.library_load_name()
    }

    #[must_use]
    pub fn is_builtin(&self) -> bool {
        self.provenance.is_builtin()
    }

    #[must_use]
    pub fn registration_module(&self) -> &str {
        self.provenance.registration_module()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledTemplateFilter {
    pub(crate) name: String,
    pub(crate) provenance: InstalledSymbolProvenance,
    pub(crate) defining_module: String,
    pub(crate) doc: Option<String>,
}

impl InstalledTemplateFilter {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn provenance(&self) -> &InstalledSymbolProvenance {
        &self.provenance
    }

    #[must_use]
    pub fn defining_module(&self) -> &str {
        &self.defining_module
    }

    #[must_use]
    pub fn doc(&self) -> Option<&str> {
        self.doc.as_deref()
    }

    #[must_use]
    pub fn library_load_name(&self) -> Option<&str> {
        self.provenance.library_load_name()
    }

    #[must_use]
    pub fn is_builtin(&self) -> bool {
        self.provenance.is_builtin()
    }

    #[must_use]
    pub fn registration_module(&self) -> &str {
        self.provenance.registration_module()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstalledSymbolProvenance {
    Library { load_name: String, module: String },
    Builtin { module: String },
}

impl InstalledSymbolProvenance {
    #[must_use]
    pub fn library_load_name(&self) -> Option<&str> {
        match self {
            Self::Library { load_name, .. } => Some(load_name),
            Self::Builtin { .. } => None,
        }
    }

    #[must_use]
    pub fn is_builtin(&self) -> bool {
        matches!(self, Self::Builtin { .. })
    }

    pub(crate) fn registration_module(&self) -> &str {
        match self {
            Self::Library { module, .. } | Self::Builtin { module } => module,
        }
    }
}

use std::collections::BTreeMap;
use std::collections::HashMap;

use camino::Utf8PathBuf;
use serde::Deserialize;
use serde::Serialize;

/// A Django template library discovered by scanning `templatetags/` directories across `sys.path`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredTemplateLibrary {
    /// The load name used in `{% load X %}` (derived from filename stem).
    pub load_name: String,
    /// The dotted Python module path of the containing app (e.g., `django.contrib.humanize`).
    pub app_module: String,
    /// The dotted Python module path of the templatetags file
    /// (e.g., `django.contrib.humanize.templatetags.humanize`).
    pub module_path: String,
    /// Absolute path to the `templatetags/*.py` source file.
    pub source_path: Utf8PathBuf,
    /// Tag names registered in this library (empty if parse failed).
    pub tags: Vec<String>,
    /// Filter names registered in this library (empty if parse failed).
    pub filters: Vec<String>,
}

/// A tag or filter name found in the environment, annotated with the library that provides it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredTemplateLibrarySymbol {
    /// The tag or filter name.
    pub name: String,
    /// The load name of the library providing this symbol.
    pub library_load_name: String,
    /// The app module that must be in `INSTALLED_APPS`.
    pub app_module: String,
}

/// All template-tag libraries discovered in the Python environment.
///
/// Built by scanning `sys.path` entries for `*/templatetags/*.py` files.
/// This is a superset of the inspector inventory — it includes libraries from
/// apps that may not be in `INSTALLED_APPS`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredTemplateLibraries {
    /// Map from load name → list of libraries (Vec because name collisions across packages are possible).
    libraries: BTreeMap<String, Vec<DiscoveredTemplateLibrary>>,
}

impl DiscoveredTemplateLibraries {
    /// Create a new `DiscoveredTemplateLibraries` from the given library map.
    #[must_use]
    pub fn new(libraries: BTreeMap<String, Vec<DiscoveredTemplateLibrary>>) -> Self {
        Self { libraries }
    }

    /// All discovered libraries, grouped by load name.
    #[must_use]
    pub fn libraries(&self) -> &BTreeMap<String, Vec<DiscoveredTemplateLibrary>> {
        &self.libraries
    }

    /// Whether a library with the given load name exists in the environment.
    #[must_use]
    pub fn has_library(&self, name: &str) -> bool {
        self.libraries.contains_key(name)
    }

    /// Get all libraries registered under a given load name.
    #[must_use]
    pub fn libraries_for_name(&self, name: &str) -> &[DiscoveredTemplateLibrary] {
        self.libraries
            .get(name)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// Total number of distinct load names.
    #[must_use]
    pub fn len(&self) -> usize {
        self.libraries.len()
    }

    /// Whether the collection is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.libraries.is_empty()
    }

    /// Reverse lookup: for each tag name, list all environment libraries providing it.
    #[must_use]
    pub fn tags_by_name(&self) -> HashMap<String, Vec<DiscoveredTemplateLibrarySymbol>> {
        let mut map: HashMap<String, Vec<DiscoveredTemplateLibrarySymbol>> = HashMap::new();
        for libs in self.libraries.values() {
            for lib in libs {
                for tag_name in &lib.tags {
                    map.entry(tag_name.clone()).or_default().push(
                        DiscoveredTemplateLibrarySymbol {
                            name: tag_name.clone(),
                            library_load_name: lib.load_name.clone(),
                            app_module: lib.app_module.clone(),
                        },
                    );
                }
            }
        }
        map
    }

    /// Reverse lookup: for each filter name, list all environment libraries providing it.
    #[must_use]
    pub fn filters_by_name(&self) -> HashMap<String, Vec<DiscoveredTemplateLibrarySymbol>> {
        let mut map: HashMap<String, Vec<DiscoveredTemplateLibrarySymbol>> = HashMap::new();
        for libs in self.libraries.values() {
            for lib in libs {
                for filter_name in &lib.filters {
                    map.entry(filter_name.clone()).or_default().push(
                        DiscoveredTemplateLibrarySymbol {
                            name: filter_name.clone(),
                            library_load_name: lib.load_name.clone(),
                            app_module: lib.app_module.clone(),
                        },
                    );
                }
            }
        }
        map
    }
}

use camino::Utf8PathBuf;
use djls_conf::DiagnosticsConfig;
use djls_extraction::FilterArity;
use djls_extraction::SymbolKey;
use djls_project::InspectorInventory;
use djls_templates::Db as TemplateDb;
use rustc_hash::FxHashMap;

use crate::blocks::TagIndex;
use crate::errors::ValidationError;
use crate::templatetags::TagSpecs;

/// Filter arity specs keyed by `SymbolKey` for collision-safe lookup.
///
/// Populated from M5 extraction results (workspace + external modules).
/// Used by M6 filter arity validation to check argument requirements.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FilterAritySpecs {
    specs: FxHashMap<SymbolKey, FilterArity>,
}

impl FilterAritySpecs {
    #[must_use]
    pub fn new(specs: FxHashMap<SymbolKey, FilterArity>) -> Self {
        Self { specs }
    }

    #[must_use]
    pub fn get(&self, key: &SymbolKey) -> Option<&FilterArity> {
        self.specs.get(key)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }
}

/// Opaque tag map: opener name → closer name.
///
/// Derived from `TagSpecs` where `spec.opaque == true`.
/// Used to identify regions where validation should be skipped
/// (e.g., inside `{% verbatim %}...{% endverbatim %}`).
pub type OpaqueTagMap = FxHashMap<String, String>;

#[salsa::db]
pub trait Db: TemplateDb {
    /// Get the Django tag specifications for semantic analysis
    fn tag_specs(&self) -> TagSpecs;

    fn tag_index(&self) -> TagIndex<'_>;

    fn template_dirs(&self) -> Option<Vec<Utf8PathBuf>>;

    /// Get the diagnostics configuration
    fn diagnostics_config(&self) -> DiagnosticsConfig;

    /// Get the inspector inventory of template tags and filters (from Python runtime).
    ///
    /// Returns `None` when the inspector is unavailable (Django not initialized,
    /// Python env not configured, inspector crashed).
    fn inspector_inventory(&self) -> Option<InspectorInventory>;

    /// Get filter arity specs keyed by `SymbolKey` (M6).
    ///
    /// Keyed by `SymbolKey { registration_module, name, kind=Filter }` for
    /// collision-safe lookup when multiple libraries define the same filter name.
    ///
    /// Returns empty `FilterAritySpecs` if extraction unavailable.
    fn filter_arity_specs(&self) -> FilterAritySpecs;

    /// Get opaque tag mappings: opener → closer (M6).
    ///
    /// Derived from `TagSpecs` where `spec.opaque == true`.
    /// Used to identify regions where validation should be skipped
    /// (e.g., inside `{% verbatim %}...{% endverbatim %}`).
    fn opaque_tag_map(&self) -> OpaqueTagMap;
}

#[salsa::accumulator]
pub struct ValidationErrorAccumulator(pub ValidationError);

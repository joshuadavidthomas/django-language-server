use djls_project::FilterArity;
use djls_project::FilterArityMap;
use djls_project::Project;
use djls_project::SymbolKey;
use djls_project::extract_filter_arities;
use djls_project::template_libraries;
use rustc_hash::FxHashMap;

use crate::db::Db;

/// Map from filter name → `FilterArity`, resolved for the current project.
///
/// Built from extraction results. For builtin filters that appear in multiple
/// modules, the last one wins (matching Django's `engine.template_builtins`
/// iteration order where later entries override earlier ones).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FilterAritySpecs {
    /// Maps filter name → (`SymbolKey`, `FilterArity`).
    /// The `SymbolKey` is retained for diagnostics / provenance tracking.
    specs: FxHashMap<String, (SymbolKey, FilterArity)>,
}

impl FilterAritySpecs {
    #[must_use]
    pub fn empty_ref() -> &'static Self {
        static EMPTY: std::sync::LazyLock<FilterAritySpecs> =
            std::sync::LazyLock::new(FilterAritySpecs::new);
        &EMPTY
    }

    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a filter arity spec. If the filter name already exists,
    /// it is overwritten (last-wins semantics for builtins).
    pub fn insert(&mut self, key: SymbolKey, arity: FilterArity) {
        self.specs.insert(key.name.clone(), (key, arity));
    }

    /// Look up the arity for a filter by name.
    #[must_use]
    pub(crate) fn get(&self, filter_name: &str) -> Option<&FilterArity> {
        self.specs.get(filter_name).map(|(_, arity)| arity)
    }

    /// Merge extracted filter arities into this map.
    /// Later entries overwrite earlier ones (last-wins).
    pub fn merge_filter_arities(&mut self, filter_arities: &FilterArityMap) {
        for (key, arity) in filter_arities {
            self.insert(key.clone(), arity.clone());
        }
    }
}

/// Compute `FilterAritySpecs` from a project's extraction results.
///
/// Merges filter arity data from discovered template tag modules, with
/// last-wins semantics for name collisions.
#[salsa::tracked(returns(ref))]
pub fn compute_filter_arity_specs(db: &dyn Db, project: Project) -> FilterAritySpecs {
    let mut specs = FilterAritySpecs::new();

    for library in template_libraries(db, project).active_libraries() {
        let extraction = extract_filter_arities(db, library.file(), library.module_name().clone());
        let filter_arities = extraction.arities();
        if !filter_arities.is_empty() {
            specs.merge_filter_arities(filter_arities);
        }
    }

    specs
}

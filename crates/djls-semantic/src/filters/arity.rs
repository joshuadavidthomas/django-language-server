use djls_extraction::FilterArity;
use djls_extraction::SymbolKey;
use rustc_hash::FxHashMap;

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
    pub fn get(&self, filter_name: &str) -> Option<&FilterArity> {
        self.specs.get(filter_name).map(|(_, arity)| arity)
    }

    /// Check if any specs are present.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }

    /// Number of filter arity specs.
    #[must_use]
    pub fn len(&self) -> usize {
        self.specs.len()
    }

    /// Merge extraction results' filter arities into this map.
    /// Later entries overwrite earlier ones (last-wins).
    pub fn merge_extraction_result(&mut self, result: &djls_extraction::ExtractionResult) {
        for (key, arity) in &result.filter_arities {
            self.insert(key.clone(), arity.clone());
        }
    }
}

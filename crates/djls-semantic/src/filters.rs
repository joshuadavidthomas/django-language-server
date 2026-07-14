use djls_project::EffectiveDefinitionLibrary;
use djls_project::FilterArity;
use djls_project::FilterArityMap;
use djls_project::SymbolKey;
use djls_project::TemplateLibraryKey;
use djls_project::TemplateSymbolKind;
use djls_project::extract_filter_arities;
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
    fn get(&self, filter_name: &str) -> Option<&FilterArity> {
        self.specs.get(filter_name).map(|(_, arity)| arity)
    }

    #[must_use]
    pub(crate) fn contains(&self, filter_name: &str) -> bool {
        self.specs.contains_key(filter_name)
    }

    /// Merge extracted filter arities into this map.
    /// Later entries overwrite earlier ones (last-wins).
    pub fn merge_filter_arities(&mut self, filter_arities: &FilterArityMap) {
        for (key, arity) in filter_arities {
            self.insert(key.clone(), arity.clone());
        }
    }
}

/// Independently backdatable semantic Filter facts for one Template Library.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LibraryFilterSpecs(FilterAritySpecs);

impl LibraryFilterSpecs {
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&FilterArity> {
        self.0.get(name)
    }
}

#[salsa::tracked(returns(ref))]
pub fn library_filter_specs(db: &dyn Db, key: TemplateLibraryKey) -> LibraryFilterSpecs {
    let extraction = extract_filter_arities(db, key);
    let mut specs = FilterAritySpecs::new();
    specs.merge_filter_arities(extraction.arities());
    LibraryFilterSpecs(specs)
}

/// Return the effective filter arity at one occurrence when every feasible backend agrees.
pub(crate) fn effective_filter_arity_in_scope(
    db: &dyn Db,
    scope_file: djls_source::File,
    filter_name: &str,
    load_state: &crate::scoping::LoadState<'_>,
) -> Option<FilterArity> {
    if db.project().is_none() {
        return db
            .projectless_filter_arity_specs()
            .get(filter_name)
            .cloned();
    }
    let loaded = load_state.libraries_loading_symbol(filter_name);
    let alternatives = crate::db::template_environment_for_file(db, scope_file)
        .effective_definition_libraries(filter_name, TemplateSymbolKind::Filter, &loaded);
    let definitions: Option<Vec<Option<FilterArity>>> = alternatives
        .into_iter()
        .map(|alternative| match alternative {
            EffectiveDefinitionLibrary::Known(library) => Some(library.and_then(|library| {
                library_filter_specs(db, library.key(db))
                    .get(filter_name)
                    .cloned()
            })),
            EffectiveDefinitionLibrary::Unobserved(library) => {
                library_filter_specs(db, library.key(db))
                    .get(filter_name)
                    .cloned()
                    .map(Some)
            }
            EffectiveDefinitionLibrary::Unknown => None,
        })
        .collect();
    let definitions = definitions?;
    let first = definitions.first()?.as_ref()?;
    definitions
        .iter()
        .all(|definition| definition.as_ref() == Some(first))
        .then(|| first.clone())
}

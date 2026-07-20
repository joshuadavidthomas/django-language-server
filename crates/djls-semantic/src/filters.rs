use std::sync::LazyLock;

use djls_project::EffectiveDefinitionLibrary;
use djls_project::FilterArity;
use djls_project::FilterArityMap;
use djls_project::ScopedTemplateLibraries;
use djls_project::SymbolKey;
use djls_project::TemplateLibraryId;
use djls_project::TemplateSymbolKind;
use djls_project::template_library_filter_facts;
use rustc_hash::FxHashMap;

use crate::db::Db;
use crate::scoping::LoadState;

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
        static EMPTY: LazyLock<FilterAritySpecs> = LazyLock::new(FilterAritySpecs::new);
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
pub fn library_filter_specs(db: &dyn Db, key: TemplateLibraryId) -> LibraryFilterSpecs {
    let facts = template_library_filter_facts(db, key);
    let mut specs = FilterAritySpecs::new();
    specs.merge_filter_arities(facts.filter_arities());
    LibraryFilterSpecs(specs)
}

/// Return the effective filter arity at one occurrence when every feasible backend agrees.
pub(crate) fn effective_filter_arity_in_scope(
    db: &dyn Db,
    scoped_libraries: ScopedTemplateLibraries<'_>,
    filter_name: &str,
    load_state: &LoadState<'_>,
) -> Option<FilterArity> {
    let loaded = load_state.libraries_loading_symbol(filter_name);
    let mut agreed = None;
    let mut alternatives_agree = true;
    scoped_libraries.for_each_effective_definition_library(
        filter_name,
        TemplateSymbolKind::Filter,
        &loaded,
        |alternative| {
            let definition = match alternative {
                EffectiveDefinitionLibrary::Known(library) => library
                    .and_then(|library| library_filter_specs(db, library.id()).get(filter_name)),
                EffectiveDefinitionLibrary::Unobserved(library) => {
                    let Some(arity) = library_filter_specs(db, library.id()).get(filter_name)
                    else {
                        alternatives_agree = false;
                        return;
                    };
                    Some(arity)
                }
                EffectiveDefinitionLibrary::Unknown => {
                    alternatives_agree = false;
                    return;
                }
            };
            match agreed {
                None => agreed = Some(definition),
                Some(existing) if existing == definition => {}
                Some(_) => alternatives_agree = false,
            }
        },
    );

    alternatives_agree
        .then_some(agreed.flatten())
        .flatten()
        .cloned()
}

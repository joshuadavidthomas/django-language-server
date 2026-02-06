//! Filter arity validation using M4 scoping + M5 extraction.
//!
//! Checks that filters are called with the correct number of arguments:
//! - S115: Filter requires an argument but none provided
//! - S116: Filter does not accept an argument but one was provided

use djls_extraction::FilterArity;
use djls_extraction::SymbolKey;
use djls_project::FilterProvenance;
use djls_templates::Node;
use salsa::Accumulator;

use crate::db::ValidationErrorAccumulator;
use crate::errors::ValidationError;
use crate::load_resolution::compute_loaded_libraries;
use crate::load_resolution::LoadState;
use crate::load_resolution::LoadedLibraries;
use crate::opaque::compute_opaque_regions;
use crate::Db;

/// Build `LoadState` for symbols available at a given position.
///
/// Processes load statements in document order up to `position`,
/// using the same state-machine approach as M3/M4.
fn build_load_state_at(loaded: &LoadedLibraries, position: u32) -> LoadState {
    let mut state = LoadState::default();
    for stmt in loaded.loads() {
        if stmt.span.end() <= position {
            state.process(stmt);
        }
    }
    state
}

/// Resolve which filter implementation is in scope at a given position.
///
/// Returns a `SymbolKey` if the filter can be unambiguously resolved, `None` otherwise.
///
/// Resolution rules:
/// - Builtin filters are always available as fallback
/// - Library filters require a loaded library or selective import
/// - If a library filter is loaded, it shadows any builtin of the same name
/// - If multiple library filters are loaded (ambiguous), returns `None`
/// - Among multiple builtins, the one from the last module in `builtins()` wins
fn resolve_filter_symbol(
    filter_name: &str,
    position: u32,
    loaded: &LoadedLibraries,
    db: &dyn Db,
) -> Option<SymbolKey> {
    let inventory = db.inspector_inventory()?;

    let state = build_load_state_at(loaded, position);

    let mut library_candidates: Vec<String> = Vec::new();
    let mut builtin_candidates: Vec<String> = Vec::new();

    for filter in inventory.iter_filters() {
        if filter.name() != filter_name {
            continue;
        }

        match filter.provenance() {
            FilterProvenance::Builtin { module } => {
                builtin_candidates.push(module.clone());
            }
            FilterProvenance::Library { load_name, module } => {
                if state.is_tag_available(filter_name, load_name) {
                    library_candidates.push(module.clone());
                }
            }
        }
    }

    // Library filters take precedence over builtins
    if library_candidates.is_empty() {
        // Fall back to builtins: later in builtins() wins (Django semantics)
        let builtin_module = builtin_candidates.into_iter().max_by_key(|m| {
            inventory
                .builtins()
                .iter()
                .position(|b| b == m)
                .unwrap_or(0)
        });

        builtin_module.map(|m| SymbolKey::filter(m, filter_name))
    } else if library_candidates.len() == 1 {
        Some(SymbolKey::filter(
            library_candidates.remove(0),
            filter_name,
        ))
    } else {
        // Ambiguous: multiple loaded libraries define this filter.
        // M4's S113 handles diagnostics; we emit no arity errors.
        None
    }
}

/// Validate filter arity for all filters in the nodelist.
///
/// Iterates over `Node::Variable` nodes, resolves each filter's symbol
/// using the load-state at its position, then checks the extracted arity.
///
/// Skips filters:
/// - Inside opaque regions (e.g., `{% verbatim %}`)
/// - That can't be resolved (unknown, ambiguous, no inventory)
/// - With no extracted arity information
/// - With `Optional` or `Unknown` arity (always valid)
pub fn validate_filter_arity(db: &dyn Db, nodelist: djls_templates::NodeList<'_>) {
    let arity_specs = db.filter_arity_specs();
    if arity_specs.is_empty() {
        return;
    }

    let opaque = compute_opaque_regions(db, nodelist);
    let loaded = compute_loaded_libraries(db, nodelist);

    for node in nodelist.nodelist(db) {
        if let Node::Variable {
            filters, span: var_span, ..
        } = node
        {
            if opaque.is_opaque(*var_span) {
                continue;
            }

            for filter in filters {
                if opaque.is_opaque(filter.span) {
                    continue;
                }

                let Some(symbol_key) =
                    resolve_filter_symbol(&filter.name, filter.span.start(), &loaded, db)
                else {
                    continue;
                };

                let Some(arity) = arity_specs.get(&symbol_key) else {
                    continue;
                };

                match arity {
                    FilterArity::None => {
                        if filter.arg.is_some() {
                            ValidationErrorAccumulator(
                                ValidationError::FilterUnexpectedArgument {
                                    filter: filter.name.clone(),
                                    span: filter.span,
                                },
                            )
                            .accumulate(db);
                        }
                    }
                    FilterArity::Required => {
                        if filter.arg.is_none() {
                            ValidationErrorAccumulator(ValidationError::FilterMissingArgument {
                                filter: filter.name.clone(),
                                span: filter.span,
                            })
                            .accumulate(db);
                        }
                    }
                    FilterArity::Optional | FilterArity::Unknown => {}
                }
            }
        }
    }
}

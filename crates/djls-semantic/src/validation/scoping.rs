use djls_project::MissingLibraryLookup;
use djls_source::Span;
use djls_templates::Filter;
use djls_templates::TagDelimiter;
use salsa::Accumulator;

use crate::db::Db;
use crate::db::ValidationErrorAccumulator;
use crate::errors::ValidationError;
use crate::scoping::symbols::SymbolAvailability;

pub(crate) fn check_tag_scoping_rule(
    db: &dyn Db,
    name: &str,
    span: Span,
    availability: &SymbolAvailability,
    unknown_load_can_supply_symbol: bool,
) {
    let full_span = span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);

    match availability {
        SymbolAvailability::Available | SymbolAvailability::Inconclusive => {}
        SymbolAvailability::Unknown => {
            ValidationErrorAccumulator(ValidationError::UnknownTag {
                tag: name.to_string(),
                span: full_span,
            })
            .accumulate(db);
        }
        SymbolAvailability::NotInInstalledApps { app, load_name } => {
            ValidationErrorAccumulator(ValidationError::TagNotInInstalledApps {
                tag: name.to_string(),
                app: app.clone(),
                load_name: load_name.clone(),
                span: full_span,
            })
            .accumulate(db);
        }
        SymbolAvailability::Unloaded { .. } | SymbolAvailability::AmbiguousUnloaded { .. }
            if unknown_load_can_supply_symbol => {}
        SymbolAvailability::Unloaded { library } => {
            ValidationErrorAccumulator(ValidationError::UnloadedTag {
                tag: name.to_string(),
                library: library.clone(),
                span: full_span,
            })
            .accumulate(db);
        }
        SymbolAvailability::AmbiguousUnloaded { libraries } => {
            ValidationErrorAccumulator(ValidationError::AmbiguousUnloadedTag {
                tag: name.to_string(),
                libraries: libraries.clone(),
                span: full_span,
            })
            .accumulate(db);
        }
    }
}

pub(crate) fn check_filter_scoping_rule(
    db: &dyn Db,
    filter: &Filter,
    availability: &SymbolAvailability,
    unknown_load_can_supply_symbol: bool,
) {
    match availability {
        SymbolAvailability::Available | SymbolAvailability::Inconclusive => {}
        SymbolAvailability::Unknown => {
            ValidationErrorAccumulator(ValidationError::UnknownFilter {
                filter: filter.name.clone(),
                span: filter.span,
            })
            .accumulate(db);
        }
        SymbolAvailability::NotInInstalledApps { app, load_name } => {
            ValidationErrorAccumulator(ValidationError::FilterNotInInstalledApps {
                filter: filter.name.clone(),
                app: app.clone(),
                load_name: load_name.clone(),
                span: filter.span,
            })
            .accumulate(db);
        }
        SymbolAvailability::Unloaded { .. } | SymbolAvailability::AmbiguousUnloaded { .. }
            if unknown_load_can_supply_symbol => {}
        SymbolAvailability::Unloaded { library } => {
            ValidationErrorAccumulator(ValidationError::UnloadedFilter {
                filter: filter.name.clone(),
                library: library.clone(),
                span: filter.span,
            })
            .accumulate(db);
        }
        SymbolAvailability::AmbiguousUnloaded { libraries } => {
            ValidationErrorAccumulator(ValidationError::AmbiguousUnloadedFilter {
                filter: filter.name.clone(),
                libraries: libraries.clone(),
                span: filter.span,
            })
            .accumulate(db);
        }
    }
}

pub(crate) fn check_load_libraries_rule(
    db: &dyn Db,
    arguments: &[crate::scoping::LoaderArgumentFact],
) {
    for fact in arguments {
        let lib = &fact.argument;
        match &fact.availability {
            MissingLibraryLookup::Inconclusive => {}
            MissingLibraryLookup::FoundInApps(apps) => {
                let candidates = apps
                    .as_slice()
                    .iter()
                    .map(|app| app.as_str().to_string())
                    .collect();
                ValidationErrorAccumulator(ValidationError::LibraryNotInInstalledApps {
                    name: lib.as_str().to_string(),
                    app: apps.primary().as_str().to_string(),
                    candidates,
                    span: lib.span(),
                })
                .accumulate(db);
            }
            MissingLibraryLookup::Absent => {
                ValidationErrorAccumulator(ValidationError::UnknownLibrary {
                    name: lib.as_str().to_string(),
                    span: lib.span(),
                })
                .accumulate(db);
            }
        }
    }
}

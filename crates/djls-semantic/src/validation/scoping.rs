use djls_project::EnvironmentSymbolLookup;
use djls_project::LibraryName;
use djls_project::MissingLibraryLookup;
use djls_project::TemplateSymbolKind;
use djls_project::TemplateSymbolLookup;
use djls_source::Span;
use djls_templates::Filter;
use djls_templates::TagBit;
use djls_templates::TagDelimiter;
use salsa::Accumulator;

use crate::db::Db;
use crate::db::ValidationErrorAccumulator;
use crate::errors::ValidationError;
use crate::scoping::symbols::AvailableSymbols;
use crate::scoping::symbols::SymbolAvailability;

/// Internal helper for [`TemplateValidator`](crate::validation::TemplateValidator).
pub(crate) fn check_tag_scoping_rule(
    db: &dyn Db,
    name: &str,
    span: Span,
    symbols: &AvailableSymbols,
    environment: djls_project::TemplateEnvironment<'_>,
    unknown_load_can_supply_symbol: bool,
) {
    let full_span = span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);

    match symbols.check_tag(name) {
        SymbolAvailability::Available => {}
        SymbolAvailability::Unknown => {
            match environment.symbol(name, TemplateSymbolKind::Tag) {
                EnvironmentSymbolLookup::Builtin
                | EnvironmentSymbolLookup::RequiresLoad(_)
                | EnvironmentSymbolLookup::Inconclusive => return,
                EnvironmentSymbolLookup::Absent => {}
            }
            match environment.available_app_symbol(name, TemplateSymbolKind::Tag) {
                TemplateSymbolLookup::Inconclusive => {}
                TemplateSymbolLookup::FoundInApp { app, load_name } => {
                    ValidationErrorAccumulator(ValidationError::TagNotInInstalledApps {
                        tag: name.to_string(),
                        app: app.as_str().to_string(),
                        load_name: load_name.as_str().to_string(),
                        span: full_span,
                    })
                    .accumulate(db);
                }
                TemplateSymbolLookup::Absent => {
                    ValidationErrorAccumulator(ValidationError::UnknownTag {
                        tag: name.to_string(),
                        span: full_span,
                    })
                    .accumulate(db);
                }
            }
        }
        SymbolAvailability::Unloaded { .. } | SymbolAvailability::AmbiguousUnloaded { .. }
            if unknown_load_can_supply_symbol => {}
        SymbolAvailability::Unloaded { library } => {
            ValidationErrorAccumulator(ValidationError::UnloadedTag {
                tag: name.to_string(),
                library,
                span: full_span,
            })
            .accumulate(db);
        }
        SymbolAvailability::AmbiguousUnloaded { libraries } => {
            ValidationErrorAccumulator(ValidationError::AmbiguousUnloadedTag {
                tag: name.to_string(),
                libraries,
                span: full_span,
            })
            .accumulate(db);
        }
    }
}

/// Internal helper for [`TemplateValidator`](crate::validation::TemplateValidator).
pub(crate) fn check_filter_scoping_rule(
    db: &dyn Db,
    filter: &Filter,
    symbols: &AvailableSymbols,
    environment: djls_project::TemplateEnvironment<'_>,
    unknown_load_can_supply_symbol: bool,
) {
    match symbols.check_filter(&filter.name) {
        SymbolAvailability::Available => {}
        SymbolAvailability::Unknown => {
            match environment.symbol(&filter.name, TemplateSymbolKind::Filter) {
                EnvironmentSymbolLookup::Builtin
                | EnvironmentSymbolLookup::RequiresLoad(_)
                | EnvironmentSymbolLookup::Inconclusive => return,
                EnvironmentSymbolLookup::Absent => {}
            }
            match environment.available_app_symbol(&filter.name, TemplateSymbolKind::Filter) {
                TemplateSymbolLookup::Inconclusive => {}
                TemplateSymbolLookup::FoundInApp { app, load_name } => {
                    ValidationErrorAccumulator(ValidationError::FilterNotInInstalledApps {
                        filter: filter.name.clone(),
                        app: app.as_str().to_string(),
                        load_name: load_name.as_str().to_string(),
                        span: filter.span,
                    })
                    .accumulate(db);
                }
                TemplateSymbolLookup::Absent => {
                    ValidationErrorAccumulator(ValidationError::UnknownFilter {
                        filter: filter.name.clone(),
                        span: filter.span,
                    })
                    .accumulate(db);
                }
            }
        }
        SymbolAvailability::Unloaded { .. } | SymbolAvailability::AmbiguousUnloaded { .. }
            if unknown_load_can_supply_symbol => {}
        SymbolAvailability::Unloaded { library } => {
            ValidationErrorAccumulator(ValidationError::UnloadedFilter {
                filter: filter.name.clone(),
                library,
                span: filter.span,
            })
            .accumulate(db);
        }
        SymbolAvailability::AmbiguousUnloaded { libraries } => {
            ValidationErrorAccumulator(ValidationError::AmbiguousUnloadedFilter {
                filter: filter.name.clone(),
                libraries,
                span: filter.span,
            })
            .accumulate(db);
        }
    }
}

/// Internal helper for [`TemplateValidator`](crate::validation::TemplateValidator).
pub(crate) fn check_load_libraries_rule(
    db: &dyn Db,
    name: &str,
    bits: &[TagBit],
    environment: djls_project::TemplateEnvironment<'_>,
) {
    let Some(kind) = crate::scoping::LoadKind::from_tag(name, bits) else {
        return;
    };

    for lib in kind.into_library_arguments() {
        let Ok(load_name) = LibraryName::parse(lib.as_str()) else {
            // Invalid library name string (shouldn't happen given LoadKind parser, but safety first)
            continue;
        };

        match environment.missing_library(&load_name) {
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

use djls_project::LibraryName;
use djls_project::TemplateLibraries;
use djls_project::UnknownLibraryOutcome;
use djls_project::UnknownSymbolOutcome;
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
    template_libraries: &TemplateLibraries,
) {
    if !template_libraries.has_symbol_inventory() {
        return;
    }

    let full_span = span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);

    match symbols.check_tag(name) {
        SymbolAvailability::Available => {}
        SymbolAvailability::Unknown => match template_libraries.unknown_tag_outcome(name) {
            UnknownSymbolOutcome::Suppressed => {}
            UnknownSymbolOutcome::Available { app, load_name } => {
                ValidationErrorAccumulator(ValidationError::TagNotInInstalledApps {
                    tag: name.to_string(),
                    app: app.as_str().to_string(),
                    load_name: load_name.as_str().to_string(),
                    span: full_span,
                })
                .accumulate(db);
            }
            UnknownSymbolOutcome::TrulyUnknown => {
                ValidationErrorAccumulator(ValidationError::UnknownTag {
                    tag: name.to_string(),
                    span: full_span,
                })
                .accumulate(db);
            }
        },
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
    template_libraries: &TemplateLibraries,
) {
    if !template_libraries.has_symbol_inventory() {
        return;
    }

    match symbols.check_filter(&filter.name) {
        SymbolAvailability::Available => {}
        SymbolAvailability::Unknown => {
            match template_libraries.unknown_filter_outcome(&filter.name) {
                UnknownSymbolOutcome::Suppressed => {}
                UnknownSymbolOutcome::Available { app, load_name } => {
                    ValidationErrorAccumulator(ValidationError::FilterNotInInstalledApps {
                        filter: filter.name.clone(),
                        app: app.as_str().to_string(),
                        load_name: load_name.as_str().to_string(),
                        span: filter.span,
                    })
                    .accumulate(db);
                }
                UnknownSymbolOutcome::TrulyUnknown => {
                    ValidationErrorAccumulator(ValidationError::UnknownFilter {
                        filter: filter.name.clone(),
                        span: filter.span,
                    })
                    .accumulate(db);
                }
            }
        }
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
    template_libraries: &TemplateLibraries,
) {
    if !template_libraries.has_symbol_inventory() {
        return;
    }

    let Some(kind) = crate::scoping::LoadKind::from_tag(name, bits) else {
        return;
    };

    let libs = match kind {
        crate::scoping::LoadKind::FullLoad { libraries } => libraries,
        crate::scoping::LoadKind::SelectiveImport { library, .. } => vec![library],
    };

    for lib in libs {
        let Ok(load_name) = LibraryName::parse(lib.as_str()) else {
            // Invalid library name string (shouldn't happen given LoadKind parser, but safety first)
            continue;
        };

        match template_libraries.unknown_library_outcome(&load_name) {
            UnknownLibraryOutcome::Suppressed => {}
            UnknownLibraryOutcome::AvailableInApps { primary_app, apps } => {
                let candidates = apps
                    .into_iter()
                    .map(|app| app.as_str().to_string())
                    .collect();
                ValidationErrorAccumulator(ValidationError::LibraryNotInInstalledApps {
                    name: lib.as_str().to_string(),
                    app: primary_app.as_str().to_string(),
                    candidates,
                    span: lib.span(),
                })
                .accumulate(db);
            }
            UnknownLibraryOutcome::TrulyUnknown => {
                ValidationErrorAccumulator(ValidationError::UnknownLibrary {
                    name: lib.as_str().to_string(),
                    span: lib.span(),
                })
                .accumulate(db);
            }
        }
    }
}

use djls_project::MissingTemplateLibraryLookup;
use djls_source::Span;
use djls_templates::Filter;
use djls_templates::TagDelimiter;

use crate::db::Db;
use crate::errors::ValidationError;
use crate::scoping::LoaderArgumentFact;
use crate::scoping::symbols::SymbolAvailability;

pub(crate) fn check_tag_scoping_rule(
    errors: &mut Vec<ValidationError>,
    name: &str,
    span: Span,
    availability: &SymbolAvailability,
    unknown_load_can_supply_symbol: bool,
) {
    let full_span = span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);

    match availability {
        SymbolAvailability::Available | SymbolAvailability::Inconclusive => {}
        SymbolAvailability::Unknown => {
            errors.push(ValidationError::UnknownTag {
                tag: name.to_string(),
                span: full_span,
            });
        }
        SymbolAvailability::NotInInstalledApps { app, load_name } => {
            errors.push(ValidationError::TagNotInInstalledApps {
                tag: name.to_string(),
                app: app.clone(),
                load_name: load_name.clone(),
                span: full_span,
            });
        }
        SymbolAvailability::Unloaded { .. } | SymbolAvailability::AmbiguousUnloaded { .. }
            if unknown_load_can_supply_symbol => {}
        SymbolAvailability::Unloaded { library } => {
            errors.push(ValidationError::UnloadedTag {
                tag: name.to_string(),
                library: library.clone(),
                span: full_span,
            });
        }
        SymbolAvailability::AmbiguousUnloaded { libraries } => {
            errors.push(ValidationError::AmbiguousUnloadedTag {
                tag: name.to_string(),
                libraries: libraries.clone(),
                span: full_span,
            });
        }
    }
}

pub(crate) fn check_filter_scoping_rule(
    errors: &mut Vec<ValidationError>,
    filter: &Filter,
    availability: &SymbolAvailability,
    unknown_load_can_supply_symbol: bool,
) {
    match availability {
        SymbolAvailability::Available | SymbolAvailability::Inconclusive => {}
        SymbolAvailability::Unknown => {
            errors.push(ValidationError::UnknownFilter {
                filter: filter.name.clone(),
                span: filter.span,
            });
        }
        SymbolAvailability::NotInInstalledApps { app, load_name } => {
            errors.push(ValidationError::FilterNotInInstalledApps {
                filter: filter.name.clone(),
                app: app.clone(),
                load_name: load_name.clone(),
                span: filter.span,
            });
        }
        SymbolAvailability::Unloaded { .. } | SymbolAvailability::AmbiguousUnloaded { .. }
            if unknown_load_can_supply_symbol => {}
        SymbolAvailability::Unloaded { library } => {
            errors.push(ValidationError::UnloadedFilter {
                filter: filter.name.clone(),
                library: library.clone(),
                span: filter.span,
            });
        }
        SymbolAvailability::AmbiguousUnloaded { libraries } => {
            errors.push(ValidationError::AmbiguousUnloadedFilter {
                filter: filter.name.clone(),
                libraries: libraries.clone(),
                span: filter.span,
            });
        }
    }
}

pub(crate) fn check_load_libraries_rule(
    db: &dyn Db,
    errors: &mut Vec<ValidationError>,
    arguments: &[LoaderArgumentFact],
) {
    for fact in arguments {
        let lib = &fact.argument;
        match &fact.availability {
            MissingTemplateLibraryLookup::Inconclusive => {}
            MissingTemplateLibraryLookup::FoundInApps(apps) => {
                let candidates = apps
                    .as_slice()
                    .iter()
                    .map(|app| app.as_str().to_string())
                    .collect();
                errors.push(ValidationError::LibraryNotInInstalledApps {
                    name: lib.as_str().to_string(),
                    app: apps.primary().as_str().to_string(),
                    candidates,
                    span: lib.span(),
                });
            }
            MissingTemplateLibraryLookup::Absent => {
                errors.push(ValidationError::UnknownLibrary {
                    name: lib.as_str().to_string(),
                    span: lib.span(),
                });
            }
        }

        if let Some(unreadable) = &fact.unreadable {
            let Some(error) = ValidationError::unreadable_library(
                db,
                lib.as_str().to_string(),
                lib.span(),
                unreadable.registration_file,
                unreadable.unread.clone(),
            ) else {
                continue;
            };
            errors.push(error);
        }
    }
}

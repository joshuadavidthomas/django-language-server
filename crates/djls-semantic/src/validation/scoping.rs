use djls_project::LibraryName;
use djls_project::StaticKnowledge;
use djls_project::TemplateLibraries;
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
    let knowledge = template_libraries.knowledge();
    if knowledge == StaticKnowledge::Unknown {
        return;
    }

    let full_span = span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);

    match symbols.check_tag(name) {
        SymbolAvailability::Available => {}
        SymbolAvailability::Unknown if knowledge == StaticKnowledge::Partial => {}
        SymbolAvailability::Unknown => {
            if let Some(candidate) = template_libraries.inactive_tag_candidates(name).first() {
                ValidationErrorAccumulator(ValidationError::TagNotInInstalledApps {
                    tag: name.to_string(),
                    app: candidate
                        .inactive_app()
                        .expect("inactive candidates should carry an app")
                        .as_str()
                        .to_string(),
                    load_name: candidate
                        .load_name()
                        .expect("inactive candidates should carry a load name")
                        .as_str()
                        .to_string(),
                    span: full_span,
                })
                .accumulate(db);
            } else {
                ValidationErrorAccumulator(ValidationError::UnknownTag {
                    tag: name.to_string(),
                    span: full_span,
                })
                .accumulate(db);
            }
        }
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
    let knowledge = template_libraries.knowledge();
    if knowledge == StaticKnowledge::Unknown {
        return;
    }

    match symbols.check_filter(&filter.name) {
        SymbolAvailability::Available => {}
        SymbolAvailability::Unknown if knowledge == StaticKnowledge::Partial => {}
        SymbolAvailability::Unknown => {
            if let Some(candidate) = template_libraries
                .inactive_filter_candidates(&filter.name)
                .first()
            {
                ValidationErrorAccumulator(ValidationError::FilterNotInInstalledApps {
                    filter: filter.name.clone(),
                    app: candidate
                        .inactive_app()
                        .expect("inactive candidates should carry an app")
                        .as_str()
                        .to_string(),
                    load_name: candidate
                        .load_name()
                        .expect("inactive candidates should carry a load name")
                        .as_str()
                        .to_string(),
                    span: filter.span,
                })
                .accumulate(db);
            } else {
                ValidationErrorAccumulator(ValidationError::UnknownFilter {
                    filter: filter.name.clone(),
                    span: filter.span,
                })
                .accumulate(db);
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
    if template_libraries.knowledge() == StaticKnowledge::Unknown {
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
        if template_libraries.is_loadable(&load_name) {
            continue;
        }

        if template_libraries.knowledge() == StaticKnowledge::Known {
            let candidates = template_libraries.inactive_library_candidates(&load_name);
            if let Some(first) = candidates.first() {
                let mut apps: Vec<_> = candidates
                    .iter()
                    .filter_map(|candidate| candidate.inactive_app())
                    .map(|app| app.as_str().to_string())
                    .collect();
                apps.dedup();
                ValidationErrorAccumulator(ValidationError::LibraryNotInInstalledApps {
                    name: lib.as_str().to_string(),
                    app: first
                        .inactive_app()
                        .expect("inactive candidates should carry an app")
                        .as_str()
                        .to_string(),
                    candidates: apps,
                    span: lib.span(),
                })
                .accumulate(db);
            } else {
                ValidationErrorAccumulator(ValidationError::UnknownLibrary {
                    name: lib.as_str().to_string(),
                    span: lib.span(),
                })
                .accumulate(db);
            }
        }
    }
}

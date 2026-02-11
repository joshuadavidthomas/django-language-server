use std::collections::HashMap;

use djls_project::LibraryName;
use djls_source::Span;
use djls_templates::tokens::TagDelimiter;
use djls_templates::Filter;
use salsa::Accumulator;

use crate::db::Db;
use crate::scoping::symbols::AvailableSymbols;
use crate::scoping::symbols::FilterAvailability;
use crate::scoping::symbols::TagAvailability;
use crate::specs::tags::TagSpecs;
use crate::ValidationError;
use crate::ValidationErrorAccumulator;

/// Internal helper for [`TemplateValidator`](crate::validation::TemplateValidator).
pub(crate) fn check_tag_scoping_rule(
    db: &dyn Db,
    name: &str,
    span: Span,
    symbols: &AvailableSymbols,
    env_tags: &Option<
        HashMap<djls_project::TemplateSymbolName, Vec<djls_project::DiscoveredSymbolCandidate>>,
    >,
) {
    let template_libraries = db.template_libraries();
    if template_libraries.inspector_knowledge != djls_project::Knowledge::Known {
        return;
    }

    let marker_span = span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);

    match symbols.check(name) {
        TagAvailability::Available => {}
        TagAvailability::Unknown => {
            if let Some(env_tags) = env_tags {
                if let Ok(key) = djls_project::TemplateSymbolName::parse(name) {
                    if let Some(env_symbols) = env_tags.get(&key) {
                        let sym = &env_symbols[0];
                        ValidationErrorAccumulator(ValidationError::TagNotInInstalledApps {
                            tag: name.to_string(),
                            app: sym.app_module.as_str().to_string(),
                            load_name: sym.library_name.as_str().to_string(),
                            span: marker_span,
                        })
                        .accumulate(db);
                        return;
                    }
                }
            }
            ValidationErrorAccumulator(ValidationError::UnknownTag {
                tag: name.to_string(),
                span: marker_span,
            })
            .accumulate(db);
        }
        TagAvailability::Unloaded { library } => {
            ValidationErrorAccumulator(ValidationError::UnloadedTag {
                tag: name.to_string(),
                library,
                span: marker_span,
            })
            .accumulate(db);
        }
        TagAvailability::AmbiguousUnloaded { libraries } => {
            ValidationErrorAccumulator(ValidationError::AmbiguousUnloadedTag {
                tag: name.to_string(),
                libraries,
                span: marker_span,
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
    env_filters: &Option<
        HashMap<djls_project::TemplateSymbolName, Vec<djls_project::DiscoveredSymbolCandidate>>,
    >,
) {
    let template_libraries = db.template_libraries();
    if template_libraries.inspector_knowledge != djls_project::Knowledge::Known {
        return;
    }

    match symbols.check_filter(&filter.name) {
        FilterAvailability::Available => {}
        FilterAvailability::Unknown => {
            if let Some(env_filters) = env_filters {
                if let Ok(key) = djls_project::TemplateSymbolName::parse(filter.name.as_str()) {
                    if let Some(env_symbols) = env_filters.get(&key) {
                        let sym = &env_symbols[0];
                        ValidationErrorAccumulator(ValidationError::FilterNotInInstalledApps {
                            filter: filter.name.clone(),
                            app: sym.app_module.as_str().to_string(),
                            load_name: sym.library_name.as_str().to_string(),
                            span: filter.span,
                        })
                        .accumulate(db);
                        return;
                    }
                }
            }
            ValidationErrorAccumulator(ValidationError::UnknownFilter {
                filter: filter.name.clone(),
                span: filter.span,
            })
            .accumulate(db);
        }
        FilterAvailability::Unloaded { library } => {
            ValidationErrorAccumulator(ValidationError::UnloadedFilter {
                filter: filter.name.clone(),
                library,
                span: filter.span,
            })
            .accumulate(db);
        }
        FilterAvailability::AmbiguousUnloaded { libraries } => {
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
    bits: &[String],
    span: Span,
    template_libraries: &djls_project::TemplateLibraries,
) {
    if template_libraries.inspector_knowledge != djls_project::Knowledge::Known {
        return;
    }

    let Some(kind) = crate::scoping::parse_load_bits(bits) else {
        return;
    };

    let libs = match kind {
        crate::scoping::LoadKind::FullLoad { libraries } => libraries,
        crate::scoping::LoadKind::SelectiveImport { library, .. } => vec![library],
    };

    for lib in libs {
        if template_libraries
            .loadable
            .contains_key(&LibraryName::parse(&lib).unwrap())
        {
            continue;
        }

        let candidates = template_libraries.discovered_app_modules_for_library_str(&lib);
        if !candidates.is_empty() {
            let marker_span = span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);
            ValidationErrorAccumulator(ValidationError::LibraryNotInInstalledApps {
                name: lib,
                app: candidates[0].clone(),
                candidates,
                span: marker_span,
            })
            .accumulate(db);
        } else {
            let marker_span = span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);
            ValidationErrorAccumulator(ValidationError::UnknownLibrary {
                name: lib,
                span: marker_span,
            })
            .accumulate(db);
        }
    }
}

pub(crate) fn is_closer_or_intermediate(name: &str, tag_specs: &TagSpecs) -> bool {
    tag_specs.is_closer(name) || tag_specs.is_intermediate(name)
}

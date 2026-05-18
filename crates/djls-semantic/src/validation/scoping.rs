use std::collections::HashMap;

use djls_source::Span;
use djls_templates::Filter;
use djls_templates::TagBit;
use djls_templates::TagDelimiter;
use salsa::Accumulator;

use crate::db::Db;
use crate::db::ValidationErrorAccumulator;
use crate::errors::ValidationError;
use crate::project::DiscoveredSymbolCandidate;
use crate::project::Knowledge;
use crate::project::LibraryName;
use crate::project::TemplateLibraries;
use crate::project::TemplateSymbolName;
use crate::scoping::symbols::AvailableSymbols;
use crate::scoping::symbols::FilterAvailability;
use crate::scoping::symbols::TagAvailability;
use crate::specs::tags::TagSpecs;

/// Internal helper for [`TemplateValidator`](crate::validation::TemplateValidator).
pub(crate) fn check_tag_scoping_rule(
    db: &dyn Db,
    name: &str,
    span: Span,
    symbols: &AvailableSymbols,
    env_tags: Option<&HashMap<TemplateSymbolName, Vec<DiscoveredSymbolCandidate>>>,
    active_knowledge: Knowledge,
) {
    if !active_knowledge.has_positive_facts() {
        return;
    }

    let full_span = span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);

    match symbols.check(name) {
        TagAvailability::Available => {}
        TagAvailability::Unknown => {
            if !active_knowledge.is_fully_known() {
                return;
            }
            if let Some(env_tags) = env_tags {
                if let Ok(key) = TemplateSymbolName::parse(name) {
                    if let Some(env_symbols) = env_tags.get(&key) {
                        if let Some(sym) = env_symbols.first() {
                            ValidationErrorAccumulator(ValidationError::TagNotInInstalledApps {
                                tag: name.to_string(),
                                app: sym.app_module.as_str().to_string(),
                                load_name: sym.library_name.as_str().to_string(),
                                span: full_span,
                            })
                            .accumulate(db);
                            return;
                        }
                    }
                }
            }
            ValidationErrorAccumulator(ValidationError::UnknownTag {
                tag: name.to_string(),
                span: full_span,
            })
            .accumulate(db);
        }
        TagAvailability::Unloaded { library } => {
            ValidationErrorAccumulator(ValidationError::UnloadedTag {
                tag: name.to_string(),
                library,
                span: full_span,
            })
            .accumulate(db);
        }
        TagAvailability::AmbiguousUnloaded { libraries } => {
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
    env_filters: Option<&HashMap<TemplateSymbolName, Vec<DiscoveredSymbolCandidate>>>,
    active_knowledge: Knowledge,
) {
    if !active_knowledge.has_positive_facts() {
        return;
    }

    match symbols.check_filter(&filter.name) {
        FilterAvailability::Available => {}
        FilterAvailability::Unknown => {
            if !active_knowledge.is_fully_known() {
                return;
            }
            if let Some(env_filters) = env_filters {
                if let Ok(key) = TemplateSymbolName::parse(filter.name.as_str()) {
                    if let Some(env_symbols) = env_filters.get(&key) {
                        if let Some(sym) = env_symbols.first() {
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
    name: &str,
    bits: &[TagBit],
    template_libraries: &TemplateLibraries,
) {
    if !template_libraries.active_knowledge.has_positive_facts() {
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
        if let Ok(name) = LibraryName::parse(lib.as_str()) {
            if template_libraries.is_enabled_library(&name) {
                continue;
            }
        } else {
            // Invalid library name string (shouldn't happen given LoadKind parser, but safety first)
            continue;
        }

        if !template_libraries.active_knowledge.is_fully_known() {
            continue;
        }

        let candidates = template_libraries.discovered_app_modules_for_library_str(lib.as_str());
        if candidates.is_empty() {
            ValidationErrorAccumulator(ValidationError::UnknownLibrary {
                name: lib.as_str().to_string(),
                span: lib.span(),
            })
            .accumulate(db);
        } else {
            ValidationErrorAccumulator(ValidationError::LibraryNotInInstalledApps {
                name: lib.as_str().to_string(),
                app: candidates[0].clone(),
                candidates,
                span: lib.span(),
            })
            .accumulate(db);
        }
    }
}

pub(crate) fn is_closer_or_intermediate(name: &str, tag_specs: &TagSpecs) -> bool {
    tag_specs.values().any(|spec| {
        spec.end_tag
            .as_ref()
            .is_some_and(|end_tag| end_tag.name.as_ref() == name)
            || spec
                .intermediate_tags
                .iter()
                .any(|tag| tag.name.as_ref() == name)
    })
}

use djls_project::InactiveLibraries;
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
use crate::scoping::symbols::FilterAvailability;
use crate::scoping::symbols::TagAvailability;
use crate::tags::TagSpecs;

/// Internal helper for [`TemplateValidator`](crate::validation::TemplateValidator).
pub(crate) fn check_tag_scoping_rule(
    db: &dyn Db,
    name: &str,
    span: Span,
    symbols: &AvailableSymbols,
    inactive_libraries: &InactiveLibraries,
    knowledge: StaticKnowledge,
) {
    if knowledge == StaticKnowledge::Unknown {
        return;
    }

    let full_span = span.expand(TagDelimiter::LENGTH_U32, TagDelimiter::LENGTH_U32);

    match symbols.check(name) {
        TagAvailability::Available => {}
        TagAvailability::Unknown if knowledge == StaticKnowledge::Partial => {}
        TagAvailability::Unknown => {
            if let Some(candidate) = inactive_libraries.tag_candidates(name).first() {
                ValidationErrorAccumulator(ValidationError::TagNotInInstalledApps {
                    tag: name.to_string(),
                    app: candidate.app.as_str().to_string(),
                    load_name: candidate.name.as_str().to_string(),
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
    inactive_libraries: &InactiveLibraries,
    knowledge: StaticKnowledge,
) {
    if knowledge == StaticKnowledge::Unknown {
        return;
    }

    match symbols.check_filter(&filter.name) {
        FilterAvailability::Available => {}
        FilterAvailability::Unknown if knowledge == StaticKnowledge::Partial => {}
        FilterAvailability::Unknown => {
            if let Some(candidate) = inactive_libraries.filter_candidates(&filter.name).first() {
                ValidationErrorAccumulator(ValidationError::FilterNotInInstalledApps {
                    filter: filter.name.clone(),
                    app: candidate.app.as_str().to_string(),
                    load_name: candidate.name.as_str().to_string(),
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
    inactive_libraries: &InactiveLibraries,
) {
    if template_libraries.knowledge == StaticKnowledge::Unknown {
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
        if template_libraries.loadable.contains_key(&load_name) {
            continue;
        }

        if template_libraries.knowledge == StaticKnowledge::Known {
            let candidates = inactive_libraries.library_candidates(&load_name);
            if let Some(first) = candidates.first() {
                let mut apps: Vec<_> = candidates
                    .iter()
                    .map(|candidate| candidate.app.as_str().to_string())
                    .collect();
                apps.dedup();
                ValidationErrorAccumulator(ValidationError::LibraryNotInInstalledApps {
                    name: lib.as_str().to_string(),
                    app: first.app.as_str().to_string(),
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

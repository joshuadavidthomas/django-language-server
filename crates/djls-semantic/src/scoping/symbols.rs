use djls_project::AppTemplateSymbolLookup;
use djls_project::ScopedTemplateLibraries;
use djls_project::ScopedTemplateSymbolLookup;
use djls_project::TemplateSymbolKind;

use crate::scoping::LoadState;

/// Availability evidence resolved for one source occurrence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SymbolAvailability {
    Available,
    Unloaded {
        library: String,
    },
    AmbiguousUnloaded {
        libraries: Vec<String>,
    },
    NotInInstalledApps {
        app: String,
        load_name: String,
    },
    /// Feasible backends or an open inventory prevent a definitive answer.
    Inconclusive,
    Unknown,
}

/// Resolve only the requested name. This deliberately does not enumerate the
/// Template Library catalog or construct a complete per-Template symbol index.
#[must_use]
pub(crate) fn resolve_occurrence_availability(
    scoped_libraries: ScopedTemplateLibraries<'_>,
    load_state: &LoadState<'_>,
    name: &str,
    kind: TemplateSymbolKind,
) -> SymbolAvailability {
    match scoped_libraries.symbol(name, kind) {
        ScopedTemplateSymbolLookup::Builtin => SymbolAvailability::Available,
        ScopedTemplateSymbolLookup::RequiresLoad(required) => {
            if required
                .iter()
                .any(|library| load_state.is_symbol_available(library.as_str(), name))
            {
                return SymbolAvailability::Available;
            }
            let mut libraries = required
                .iter()
                .map(|library| library.as_str().to_string())
                .collect::<Vec<_>>();
            libraries.sort_unstable();
            libraries.dedup();
            match libraries.as_slice() {
                [] => SymbolAvailability::Unknown,
                [library] => SymbolAvailability::Unloaded {
                    library: library.clone(),
                },
                _ => SymbolAvailability::AmbiguousUnloaded { libraries },
            }
        }
        ScopedTemplateSymbolLookup::Inconclusive => SymbolAvailability::Inconclusive,
        ScopedTemplateSymbolLookup::Absent => {
            match scoped_libraries.available_in_app_symbol(name, kind) {
                AppTemplateSymbolLookup::FoundInApp { app, load_name } => {
                    SymbolAvailability::NotInInstalledApps {
                        app: app.as_str().to_string(),
                        load_name: load_name.as_str().to_string(),
                    }
                }
                AppTemplateSymbolLookup::Inconclusive => SymbolAvailability::Inconclusive,
                AppTemplateSymbolLookup::Absent => SymbolAvailability::Unknown,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use djls_project::ScopedTemplateLibraries;
    use djls_project::TemplateLibraryCatalog;
    use djls_project::TemplateSymbolKind;
    use djls_source::Span;
    use djls_testing::make_template_library_catalog;
    use serde_json::json;

    use super::*;
    use crate::scoping::LoadKind;
    use crate::scoping::LoadStatement;
    use crate::scoping::LoadedLibraries;
    use crate::scoping::loads::LoadArgument;

    fn catalog() -> TemplateLibraryCatalog {
        let db = djls_testing::TestDatabase::new();
        make_template_library_catalog(
            &db,
            &[json!({
                "kind": "tag",
                "name": "custom",
                "library_kind": "loadable",
                "load_name": "extras",
                "library_module": "example.extras",
                "module": "example.extras",
                "doc": null,
            })],
            &[],
            &HashMap::from([("extras".to_string(), "example.extras".to_string())]),
            &[],
        )
    }

    #[test]
    fn occurrence_lookup_respects_positioned_loads() {
        let catalog = catalog();
        let scoped_libraries = ScopedTemplateLibraries::from_project_inventory(&catalog);
        let loaded = LoadedLibraries::new(vec![LoadStatement::new(
            Span::new(10, 10),
            LoadKind::FullLoad {
                libraries: vec![LoadArgument::new("extras".to_string(), Span::new(13, 6))],
            },
        )]);

        assert_eq!(
            resolve_occurrence_availability(
                scoped_libraries,
                &loaded.available_at(0),
                "custom",
                TemplateSymbolKind::Tag,
            ),
            SymbolAvailability::Unloaded {
                library: "extras".to_string(),
            }
        );
        assert_eq!(
            resolve_occurrence_availability(
                scoped_libraries,
                &loaded.available_at(30),
                "custom",
                TemplateSymbolKind::Tag,
            ),
            SymbolAvailability::Available
        );
    }
}

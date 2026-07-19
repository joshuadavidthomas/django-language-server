use djls_project::EnvironmentSymbolLookup;
use djls_project::TemplateEnvironment;
use djls_project::TemplateSymbolKind;
use djls_project::TemplateSymbolLookup;

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

/// Resolve only the requested name. This deliberately does not enumerate a
/// Template Environment or construct a complete per-Template symbol index.
#[must_use]
pub(crate) fn resolve_occurrence_availability(
    environment: TemplateEnvironment<'_>,
    load_state: &LoadState<'_>,
    name: &str,
    kind: TemplateSymbolKind,
) -> SymbolAvailability {
    match environment.symbol(name, kind) {
        EnvironmentSymbolLookup::Builtin => SymbolAvailability::Available,
        EnvironmentSymbolLookup::RequiresLoad(required) => {
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
        EnvironmentSymbolLookup::Inconclusive => SymbolAvailability::Inconclusive,
        EnvironmentSymbolLookup::Absent => match environment.available_app_symbol(name, kind) {
            TemplateSymbolLookup::FoundInApp { app, load_name } => {
                SymbolAvailability::NotInInstalledApps {
                    app: app.as_str().to_string(),
                    load_name: load_name.as_str().to_string(),
                }
            }
            TemplateSymbolLookup::Inconclusive => SymbolAvailability::Inconclusive,
            TemplateSymbolLookup::Absent => SymbolAvailability::Unknown,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use djls_project::TemplateEnvironment;
    use djls_project::TemplateLibraries;
    use djls_project::TemplateSymbolKind;
    use djls_source::Span;
    use djls_testing::make_template_libraries;
    use serde_json::json;

    use super::*;
    use crate::scoping::LoadKind;
    use crate::scoping::LoadStatement;
    use crate::scoping::LoadedLibraries;
    use crate::scoping::loads::LoadArgument;

    fn environment() -> TemplateLibraries {
        let db = djls_testing::TestDatabase::new();
        make_template_libraries(
            &db,
            &[json!({
                "kind": "tag",
                "name": "custom",
                "library_kind": "installed",
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
        let libraries = environment();
        let environment = TemplateEnvironment::from_project_inventory(&libraries);
        let loaded = LoadedLibraries::new(vec![LoadStatement::new(
            Span::new(10, 10),
            LoadKind::FullLoad {
                libraries: vec![LoadArgument::new("extras".to_string(), Span::new(13, 6))],
            },
        )]);

        assert_eq!(
            resolve_occurrence_availability(
                environment,
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
                environment,
                &loaded.available_at(30),
                "custom",
                TemplateSymbolKind::Tag,
            ),
            SymbolAvailability::Available
        );
    }
}

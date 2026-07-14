use djls_source::File;

use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::templates::ContextualLibraryChain;
use crate::templates::EffectiveDefinitionLibrary;
use crate::templates::EnvironmentSymbolLookup;
use crate::templates::LibraryName;
use crate::templates::LoadableLibraryLookup;
use crate::templates::MissingLibraryLookup;
use crate::templates::TemplateLibraries;
use crate::templates::TemplateLibrary;
use crate::templates::TemplateSymbolCandidate;
use crate::templates::TemplateSymbolKind;
use crate::templates::TemplateSymbolLookup;
use crate::templates::resolution::BackendSelection;
use crate::templates::template_libraries;
use crate::templates::template_resolution;

/// Compact evidence selecting the Template backends that can render a file.
///
/// The project-inventory case is intentional: files outside configured Template roots still use
/// the independently useful project catalog rather than pretending the project has no libraries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TemplateEnvironmentScope {
    ProjectInventory,
    BackendSelections(Vec<BackendSelection>),
}

impl TemplateEnvironmentScope {
    const fn project_inventory() -> &'static Self {
        static PROJECT_INVENTORY: TemplateEnvironmentScope =
            TemplateEnvironmentScope::ProjectInventory;
        &PROJECT_INVENTORY
    }

    fn from_backend_selections(mut selections: Vec<BackendSelection>) -> Self {
        selections.sort_unstable();
        selections.dedup();
        if selections.is_empty() {
            Self::ProjectInventory
        } else {
            Self::BackendSelections(selections)
        }
    }

    pub(crate) fn backend_selections(&self) -> Option<&[BackendSelection]> {
        match self {
            Self::ProjectInventory => None,
            Self::BackendSelections(selections) => Some(selections),
        }
    }
}

/// A borrowed Template Library environment attached to a concrete template scope.
///
/// The catalog is shared by every file in the Project. A file can be feasible under more than one
/// settings configuration or backend, so the compact scope retains those alternatives and lookup
/// only makes a library or symbol definite when every feasible backend agrees.
#[derive(Clone, Copy)]
pub struct TemplateEnvironment<'db> {
    catalog: &'db TemplateLibraries,
    scope: &'db TemplateEnvironmentScope,
}

impl<'db> TemplateEnvironment<'db> {
    /// Wrap project-global inventory for commands and tests that have no concrete project file.
    #[must_use]
    pub fn from_project_inventory(catalog: &'db TemplateLibraries) -> Self {
        Self {
            catalog,
            scope: TemplateEnvironmentScope::project_inventory(),
        }
    }

    /// Resolve the definition effective in each feasible backend at one template scope.
    ///
    /// Builtins are applied in backend order, followed by the concrete `{% load %}` libraries in
    /// source order. Absence remains an explicit alternative and uncertainty is correlated with
    /// the builtin chain or load names that can actually affect this symbol.
    #[must_use]
    pub fn effective_definition_libraries(
        self,
        symbol_name: &str,
        kind: TemplateSymbolKind,
        loaded_names: &[&str],
    ) -> Vec<EffectiveDefinitionLibrary<'db>> {
        self.catalog.effective_definition_libraries_in_scope(
            self.scope,
            symbol_name,
            kind,
            loaded_names,
        )
    }

    /// Return each feasible backend's ordered builtin and load updates.
    #[must_use]
    pub fn contextual_library_chains(
        self,
        loaded_names: &[&str],
    ) -> Vec<ContextualLibraryChain<'db>> {
        self.catalog
            .contextual_library_chains_in_scope(self.scope, loaded_names)
    }

    /// Resolve a load name within the backends that can render this file.
    #[must_use]
    pub fn loadable_library(self, name: &LibraryName) -> LoadableLibraryLookup<'db> {
        self.catalog.loadable_library_in_scope(self.scope, name)
    }

    #[must_use]
    pub fn loadable_library_str(self, name: &str) -> LoadableLibraryLookup<'db> {
        self.catalog.loadable_library_str_in_scope(self.scope, name)
    }

    #[must_use]
    pub fn symbol(self, name: &str, kind: TemplateSymbolKind) -> EnvironmentSymbolLookup {
        self.catalog
            .environment_symbol_lookup_in_scope(self.scope, name, kind)
    }

    #[must_use]
    pub fn available_app_symbol(
        self,
        name: &str,
        kind: TemplateSymbolKind,
    ) -> TemplateSymbolLookup {
        self.catalog
            .template_symbol_lookup_in_scope(self.scope, name, kind)
    }

    #[must_use]
    pub fn missing_library(self, name: &LibraryName) -> MissingLibraryLookup {
        self.catalog
            .missing_library_lookup_in_scope(self.scope, name)
    }

    /// Return the concrete library identities participating in the selected environment.
    #[must_use]
    pub fn resolved_libraries(self) -> Vec<&'db TemplateLibrary> {
        self.catalog.resolved_libraries_in_scope(self.scope)
    }

    /// Whether discovery may have omitted definition names from the shared catalog.
    #[must_use]
    pub fn definition_names_are_open(self) -> bool {
        self.catalog.definition_names_are_open()
    }

    /// Return the number of builtin and loadable libraries in this environment.
    #[must_use]
    pub fn installed_library_count(self) -> usize {
        self.resolved_libraries().len()
    }

    /// Borrow every indexed symbol name from the shared Project catalog.
    ///
    /// This is inventory enumeration, not contextual availability. Callers that need symbols
    /// usable in this environment must pass each relevant name to `contextual_symbol_candidates`.
    pub fn inventory_symbol_names(
        self,
        kind: TemplateSymbolKind,
    ) -> impl Iterator<Item = &'db str> + 'db {
        self.catalog.inventory_symbol_names(kind)
    }

    /// Return the candidates for one name only when they are definite in every feasible backend.
    #[must_use]
    pub fn contextual_symbol_candidates(
        self,
        name: &str,
        kind: TemplateSymbolKind,
    ) -> Vec<TemplateSymbolCandidate> {
        self.catalog
            .contextual_symbol_candidates_in_scope(self.scope, name, kind)
    }

    #[must_use]
    pub fn completion_library_names(self) -> Vec<LibraryName> {
        self.catalog.completion_library_names_in_scope(self.scope)
    }

    /// Resolve a library link target only when every feasible backend agrees.
    #[must_use]
    pub fn library_link(self, name: &LibraryName) -> Option<File> {
        self.loadable_library(name)
            .found()
            .and_then(TemplateLibrary::source_file)
    }
}

#[must_use]
pub fn template_environment(
    db: &dyn ProjectDb,
    project: Project,
    file: File,
) -> TemplateEnvironment<'_> {
    TemplateEnvironment {
        catalog: template_libraries(db, project),
        scope: template_environment_scope(db, project, file),
    }
}

#[salsa::tracked(returns(ref))]
fn template_environment_scope(
    db: &dyn ProjectDb,
    project: Project,
    file: File,
) -> TemplateEnvironmentScope {
    let selections = template_resolution(db, project).backend_selections_for_file(db, file);
    TemplateEnvironmentScope::from_backend_selections(selections)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_selection_scope_is_canonical() {
        let first = BackendSelection::Known {
            configuration: 0,
            backend: 1,
        };
        let second = BackendSelection::Unknown { configuration: 1 };

        assert_eq!(
            TemplateEnvironmentScope::from_backend_selections(vec![second, first, second]),
            TemplateEnvironmentScope::from_backend_selections(vec![first, second]),
        );
    }
}

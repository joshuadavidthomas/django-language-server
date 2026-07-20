use djls_source::File;

use crate::db::Db as ProjectDb;
use crate::project::Project;
use crate::templates::AppTemplateSymbolLookup;
use crate::templates::EffectiveDefinitionLibrary;
use crate::templates::LibraryName;
use crate::templates::LoadableLibraryLookup;
use crate::templates::MissingTemplateLibraryLookup;
use crate::templates::ScopedTemplateSymbolLookup;
use crate::templates::TemplateBackendScope;
use crate::templates::TemplateLibrary;
use crate::templates::TemplateLibraryCatalog;
use crate::templates::TemplateLibraryChain;
use crate::templates::TemplateLibraryChainStep;
use crate::templates::TemplateSymbolCandidate;
use crate::templates::TemplateSymbolKind;
use crate::templates::template_library_catalog;
use crate::templates::template_resolution;

/// Borrowed Template Library catalog access under one Template Backend Scope.
///
/// The catalog is shared by every file in the Project. A file can be feasible under more than one
/// Template settings case or backend, so the compact scope retains those alternatives and lookup
/// only makes a library or symbol definite when every feasible backend agrees.
#[derive(Clone, Copy)]
pub struct ScopedTemplateLibraries<'db> {
    catalog: &'db TemplateLibraryCatalog,
    scope: &'db TemplateBackendScope,
}

impl<'db> ScopedTemplateLibraries<'db> {
    /// Wrap project-global inventory for commands and tests that have no concrete project file.
    #[must_use]
    pub fn from_project_inventory(catalog: &'db TemplateLibraryCatalog) -> Self {
        Self {
            catalog,
            scope: TemplateBackendScope::project_inventory_ref(),
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
    pub fn library_chains(self, loaded_names: &[&str]) -> Vec<TemplateLibraryChain<'db>> {
        self.catalog
            .library_chains_in_scope(self.scope, loaded_names)
    }

    /// Fold each feasible backend's ordered builtin and load updates without materializing chains.
    ///
    /// `initial` creates one consumer state per backend alternative, `step` receives that
    /// alternative's updates in Django precedence order, and `finish` receives the completed
    /// state. Open or omitted alternatives are represented by an explicit `Unknown` step.
    pub fn fold_library_chains<State>(
        self,
        loaded_names: &[&str],
        initial: impl FnMut() -> State,
        step: impl FnMut(&mut State, TemplateLibraryChainStep<'db>),
        finish: impl FnMut(State),
    ) {
        self.catalog
            .fold_library_chains_in_scope(self.scope, loaded_names, initial, step, finish);
    }

    /// Visit the definition effective in each feasible backend without allocating alternatives.
    pub fn for_each_effective_definition_library(
        self,
        symbol_name: &str,
        kind: TemplateSymbolKind,
        loaded_names: &[&str],
        visitor: impl FnMut(EffectiveDefinitionLibrary<'db>),
    ) {
        self.catalog.for_each_effective_definition_library_in_scope(
            self.scope,
            symbol_name,
            kind,
            loaded_names,
            visitor,
        );
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
    pub fn symbol(self, name: &str, kind: TemplateSymbolKind) -> ScopedTemplateSymbolLookup {
        self.catalog
            .scoped_symbol_lookup_in_scope(self.scope, name, kind)
    }

    #[must_use]
    pub fn available_in_app_symbol(
        self,
        name: &str,
        kind: TemplateSymbolKind,
    ) -> AppTemplateSymbolLookup {
        self.catalog
            .template_symbol_lookup_in_scope(self.scope, name, kind)
    }

    #[must_use]
    pub fn missing_library(self, name: &LibraryName) -> MissingTemplateLibraryLookup {
        self.catalog
            .missing_library_lookup_in_scope(self.scope, name)
    }

    /// Return the concrete library identities participating in the selected Template Backend Scope.
    #[must_use]
    pub fn resolved_libraries(self) -> Vec<&'db TemplateLibrary> {
        self.catalog.resolved_libraries_in_scope(self.scope)
    }

    /// Whether discovery may have omitted definition names from the shared catalog.
    #[must_use]
    pub fn definition_names_are_open(self) -> bool {
        self.catalog.definition_names_are_open()
    }

    /// Return the number of builtin and loadable libraries in this Template Backend Scope.
    #[must_use]
    pub fn resolved_library_count(self) -> usize {
        self.resolved_libraries().len()
    }

    /// Borrow every indexed symbol name from the shared Project catalog.
    ///
    /// This is inventory enumeration, not contextual availability. Callers that need symbols
    /// usable in this Template Backend Scope must pass each relevant name to `scoped_symbol_candidates`.
    pub fn inventory_symbol_names(
        self,
        kind: TemplateSymbolKind,
    ) -> impl Iterator<Item = &'db str> + 'db {
        self.catalog.inventory_symbol_names(kind)
    }

    /// Return the candidates for one name only when they are definite in every feasible backend.
    #[must_use]
    pub fn scoped_symbol_candidates(
        self,
        name: &str,
        kind: TemplateSymbolKind,
    ) -> Vec<TemplateSymbolCandidate> {
        self.catalog
            .scoped_symbol_candidates_in_scope(self.scope, name, kind)
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
pub fn scoped_template_libraries(
    db: &dyn ProjectDb,
    project: Project,
    file: File,
) -> ScopedTemplateLibraries<'_> {
    ScopedTemplateLibraries {
        catalog: template_library_catalog(db, project),
        scope: template_library_scope(db, project, file),
    }
}

#[salsa::tracked(returns(ref))]
fn template_library_scope(
    db: &dyn ProjectDb,
    project: Project,
    file: File,
) -> TemplateBackendScope {
    template_resolution(db, project).backend_scope_for_file(db, file)
}

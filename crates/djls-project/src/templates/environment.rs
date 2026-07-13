use std::collections::BTreeSet;

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
use crate::templates::TemplateSymbol;
use crate::templates::TemplateSymbolCandidate;
use crate::templates::TemplateSymbolKind;
use crate::templates::TemplateSymbolLookup;
use crate::templates::template_libraries;
use crate::templates::template_resolution;

#[derive(Clone, Copy)]
enum TemplateEnvironmentSource<'db> {
    File { project: Project, file: File },
    ProjectInventory(&'db TemplateLibraries),
}

/// The Template Library environment attached to a concrete template file.
///
/// A file can be feasible under more than one settings configuration or backend. The environment
/// retains those alternatives and only makes a library or symbol definite when every feasible
/// backend agrees.
#[derive(Clone, Copy)]
pub struct TemplateEnvironment<'db> {
    source: TemplateEnvironmentSource<'db>,
}

impl<'db> TemplateEnvironment<'db> {
    /// Wrap project-global inventory for commands and tests that have no concrete project file.
    #[must_use]
    pub fn from_project_inventory(libraries: &'db TemplateLibraries) -> Self {
        Self {
            source: TemplateEnvironmentSource::ProjectInventory(libraries),
        }
    }

    fn libraries(self, db: &'db dyn ProjectDb) -> &'db TemplateLibraries {
        match self.source {
            TemplateEnvironmentSource::File { project, file } => {
                template_environment_libraries(db, project, file)
            }
            TemplateEnvironmentSource::ProjectInventory(libraries) => libraries,
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
        db: &'db dyn ProjectDb,
        symbol_name: &str,
        kind: TemplateSymbolKind,
        loaded_names: &[&str],
    ) -> Vec<EffectiveDefinitionLibrary<'db>> {
        self.libraries(db)
            .effective_definition_libraries(symbol_name, kind, loaded_names)
    }

    /// Return each feasible backend's ordered builtin and load updates.
    #[must_use]
    pub fn contextual_library_chains(
        self,
        db: &'db dyn ProjectDb,
        loaded_names: &[&str],
    ) -> Vec<ContextualLibraryChain<'db>> {
        self.libraries(db).contextual_library_chains(loaded_names)
    }

    /// Resolve a load name within the backends that can render this file.
    #[must_use]
    pub fn loadable_library(
        self,
        db: &'db dyn ProjectDb,
        name: &LibraryName,
    ) -> LoadableLibraryLookup<'db> {
        self.libraries(db).loadable_library(name)
    }

    #[must_use]
    pub fn loadable_library_str(
        self,
        db: &'db dyn ProjectDb,
        name: &str,
    ) -> LoadableLibraryLookup<'db> {
        self.libraries(db).loadable_library_str(name)
    }

    #[must_use]
    pub fn symbol(
        self,
        db: &'db dyn ProjectDb,
        name: &str,
        kind: TemplateSymbolKind,
    ) -> EnvironmentSymbolLookup {
        self.libraries(db).environment_symbol_lookup(name, kind)
    }

    #[must_use]
    pub fn available_app_symbol(
        self,
        db: &'db dyn ProjectDb,
        name: &str,
        kind: TemplateSymbolKind,
    ) -> TemplateSymbolLookup {
        self.libraries(db).template_symbol_lookup(name, kind)
    }

    #[must_use]
    pub fn missing_library(
        self,
        db: &'db dyn ProjectDb,
        name: &LibraryName,
    ) -> MissingLibraryLookup {
        self.libraries(db).missing_library_lookup(name)
    }

    /// Return the concrete library identities participating in the selected environment.
    #[must_use]
    pub fn resolved_libraries(self, db: &'db dyn ProjectDb) -> Vec<&'db TemplateLibrary> {
        self.libraries(db).resolved_libraries().collect()
    }

    /// Return every known symbol name from the feasible backend inventory.
    #[must_use]
    pub fn candidate_symbol_names(
        self,
        db: &'db dyn ProjectDb,
        kind: TemplateSymbolKind,
    ) -> BTreeSet<String> {
        self.libraries(db).candidate_symbol_names(kind)
    }

    /// Return only tag/filter candidates that are definite in every feasible backend.
    #[must_use]
    pub fn symbol_candidates(
        self,
        db: &'db dyn ProjectDb,
        kind: TemplateSymbolKind,
    ) -> Vec<TemplateSymbolCandidate> {
        self.libraries(db).definite_template_symbol_candidates(kind)
    }

    #[must_use]
    pub fn completion_library_names(self, db: &'db dyn ProjectDb) -> Vec<LibraryName> {
        self.libraries(db).completion_library_names()
    }

    /// Return possible symbols for completion without promoting an uncertain library to found.
    #[must_use]
    pub fn loadable_symbol_candidates(
        self,
        db: &'db dyn ProjectDb,
        name: &str,
    ) -> Vec<TemplateSymbol> {
        let libraries = match self.loadable_library_str(db, name) {
            LoadableLibraryLookup::Found(library) => vec![library],
            LoadableLibraryLookup::Ambiguous(libraries)
            | LoadableLibraryLookup::Inconclusive(libraries) => libraries,
            LoadableLibraryLookup::Absent => Vec::new(),
        };
        libraries
            .into_iter()
            .flat_map(TemplateLibrary::symbols)
            .cloned()
            .collect()
    }

    /// Resolve a library link target only when every feasible backend agrees.
    #[must_use]
    pub fn library_link(self, db: &'db dyn ProjectDb, name: &LibraryName) -> Option<File> {
        self.loadable_library(db, name)
            .found()
            .and_then(TemplateLibrary::source_file)
    }
}

#[must_use]
pub fn template_environment(
    _db: &dyn ProjectDb,
    project: Project,
    file: File,
) -> TemplateEnvironment<'_> {
    TemplateEnvironment {
        source: TemplateEnvironmentSource::File { project, file },
    }
}

#[salsa::tracked(returns(ref))]
fn template_environment_libraries(
    db: &dyn ProjectDb,
    project: Project,
    file: File,
) -> TemplateLibraries {
    let libraries = template_libraries(db, project);
    let selections = template_resolution(db, project).backend_selections_for_file(db, file);

    if selections.is_empty() {
        // Files outside configured roots have no backend evidence. Retain the independently useful
        // project inventory rather than pretending the project has no Template Libraries.
        libraries.clone()
    } else {
        libraries.for_backend_selections(&selections)
    }
}

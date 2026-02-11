use djls_project::Db as ProjectDb;
use djls_project::TemplateLibrariesRequest;
use djls_project::TemplateLibrariesResponse;
use djls_project::TemplateLibrary;
use salsa::Setter;

use crate::db::DjangoDatabase;

impl DjangoDatabase {
    /// Refresh all inspector-derived data: inventory and external rules.
    ///
    /// This is a side-effect operation that bypasses Salsa tracked functions,
    /// querying the inspector subprocess directly and only calling Salsa
    /// setters when values have actually changed (Ruff/RA pattern).
    pub fn refresh_inspector(&mut self) {
        self.query_inspector_template_libraries();
        self.extract_external_rules();
    }

    /// Query the Python inspector subprocess and update the project's template libraries.
    fn query_inspector_template_libraries(&mut self) {
        let Some(project) = self.project() else {
            return;
        };

        let interpreter = project.interpreter(self).clone();
        let root = project.root(self).clone();
        let dsm = project.django_settings_module(self).clone();
        let pythonpath = project.pythonpath(self).clone();

        let response = match self
            .inspector
            .query::<TemplateLibrariesRequest, TemplateLibrariesResponse>(
                &interpreter,
                &root,
                dsm.as_deref(),
                &pythonpath,
                &TemplateLibrariesRequest,
            ) {
            Ok(response) if response.ok => response.data,
            Ok(response) => {
                tracing::warn!(
                    "query_inspector: inspector returned ok=false, error={:?}",
                    response.error
                );
                None
            }
            Err(e) => {
                tracing::error!("query_inspector: inspector query failed: {}", e);
                None
            }
        };

        let current = project.template_libraries(self).clone();
        let next = current.apply_inspector(response);
        if project.template_libraries(self) != &next {
            project.set_template_libraries(self).to(next);
        }
    }

    /// Extract validation rules from external (non-workspace) registration modules
    /// and update the project's extracted rules if they differ.
    ///
    /// Workspace modules are handled separately by `collect_workspace_extraction_results`
    /// which uses tracked Salsa queries for automatic invalidation on file change.
    fn extract_external_rules(&mut self) {
        let Some(project) = self.project() else {
            return;
        };

        let interpreter = project.interpreter(self).clone();
        let root = project.root(self).clone();
        let pythonpath = project.pythonpath(self).clone();

        let modules: rustc_hash::FxHashSet<String> = project
            .template_libraries(self)
            .registration_modules()
            .into_iter()
            .map(|m| m.as_str().to_string())
            .collect();

        let new_extraction = if modules.is_empty() {
            rustc_hash::FxHashMap::default()
        } else {
            djls_project::extract_external_rules(&modules, &interpreter, &root, &pythonpath)
        };

        if project.extracted_external_rules(self) != &new_extraction {
            project
                .set_extracted_external_rules(self)
                .to(new_extraction);
        }
    }

    /// Update the project's discovered template libraries.
    ///
    /// This is a side-effect operation that should be run off the LSP request path.
    /// It only calls Salsa setters when values have actually changed.
    pub fn update_discovered_template_libraries(&mut self, libraries: &[TemplateLibrary]) {
        let Some(project) = self.project() else {
            return;
        };

        let current = project.template_libraries(self).clone();
        let next = current.apply_discovery(libraries.iter().cloned());
        if project.template_libraries(self) != &next {
            project.set_template_libraries(self).to(next);
        }
    }
}

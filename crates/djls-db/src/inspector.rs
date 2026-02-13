use djls_project::Db as ProjectDb;
use djls_project::TemplateLibrariesRequest;
use djls_project::TemplateLibrariesResponse;
use djls_project::TemplateLibrary;
use salsa::Setter;

use crate::db::DjangoDatabase;

impl DjangoDatabase {
    /// Populate template libraries from the filesystem cache, if available.
    ///
    /// This is a fast, synchronous operation that loads a previously cached
    /// inspector response from disk. Returns `true` if the cache was loaded
    /// successfully (meaning we can defer the real inspector query to the
    /// background).
    pub fn load_inspector_cache(&mut self) -> bool {
        let Some(project) = self.project() else {
            return false;
        };

        let interpreter = project.interpreter(self).clone();
        let root = project.root(self).clone();
        let dsm = project.django_settings_module(self).clone();
        let pythonpath = project.pythonpath(self).clone();

        let Some(response) = djls_project::load_cached_inspector_response(
            &root,
            &interpreter,
            dsm.as_deref(),
            &pythonpath,
        ) else {
            return false;
        };

        let current = project.template_libraries(self).clone();
        let next = current.apply_inspector(Some(response));
        if project.template_libraries(self) != &next {
            project.set_template_libraries(self).to(next);
        }

        true
    }

    /// Refresh all inspector-derived data: inventory and external rules.
    ///
    /// Queries the Python inspector subprocess, updates Salsa inputs, extracts
    /// external rules, and writes the response to the filesystem cache for
    /// future startups.
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
        let env_vars = project.env_vars(self).clone();

        let response = match self
            .inspector
            .query::<TemplateLibrariesRequest, TemplateLibrariesResponse>(
                &interpreter,
                &root,
                dsm.as_deref(),
                &pythonpath,
                &env_vars,
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

        if let Some(ref response) = response {
            djls_project::save_inspector_response(
                &root,
                &interpreter,
                dsm.as_deref(),
                &pythonpath,
                response,
            );
        }

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

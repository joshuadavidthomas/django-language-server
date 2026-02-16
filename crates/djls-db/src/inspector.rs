use djls_project::Db as ProjectDb;
use djls_project::TemplateLibrariesRequest;
use djls_project::TemplateLibrariesResponse;
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

    /// Refresh all project-derived data: inspector inventory, external rules,
    /// and external models.
    ///
    /// Queries the Python inspector subprocess for template libraries, then
    /// scans the filesystem for external rules and model definitions.
    pub fn refresh_inspector(&mut self) {
        self.query_inspector_template_libraries();
        self.scan_external_rules();
        self.scan_external_models();
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
                if let Some(ref error) = response.error {
                    tracing::warn!("query_inspector: inspector failed: {}", error);
                } else {
                    tracing::warn!("query_inspector: inspector returned an error with no details");
                }
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
}

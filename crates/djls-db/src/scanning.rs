use djls_project::Db as ProjectDb;
use salsa::Setter;

use crate::db::DjangoDatabase;

impl DjangoDatabase {
    /// Scan the venv's site-packages for `models.py` files and extract model
    /// graphs. Updates the project's `extracted_external_models` field if the
    /// results differ from the current value.
    ///
    /// Workspace `models.py` files are handled separately by
    /// `collect_workspace_models` which uses tracked Salsa queries for
    /// automatic invalidation on file change.
    pub(crate) fn scan_external_models(&mut self) {
        let Some(project) = self.project() else {
            return;
        };

        let interpreter = project.interpreter(self).clone();
        let root = project.root(self).clone();

        let new_models = djls_project::extract_external_models(&interpreter, &root);

        if project.extracted_external_models(self) != &new_models {
            project.set_extracted_external_models(self).to(new_models);
        }
    }

    /// Extract validation rules from external (non-workspace) registration modules
    /// and update the project's extracted rules if they differ.
    ///
    /// Workspace modules are handled separately by `collect_workspace_extraction_results`
    /// which uses tracked Salsa queries for automatic invalidation on file change.
    pub(crate) fn scan_external_rules(&mut self) {
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
}

use std::sync::Arc;

use djls_conf::Settings;
use djls_project::Db as ProjectDb;

use crate::db::DjangoDatabase;

impl DjangoDatabase {
    /// Get a clone of the settings owned by this database snapshot.
    pub fn settings(&self) -> Settings {
        self.settings.as_ref().clone()
    }

    /// Store project settings and update the stable project handle.
    pub fn apply_project_settings(&mut self, settings: Settings) {
        if let Some(project) = self.project() {
            project.reload_from_settings(self, &settings);
        }

        self.settings = Arc::new(settings);
    }
}

use djls_conf::Settings;
use djls_project::Db as ProjectDb;

use crate::db::DjangoDatabase;

impl DjangoDatabase {
    /// Get a clone of the current settings.
    ///
    /// # Panics
    ///
    /// Panics if the settings mutex is poisoned.
    pub fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    /// Store project settings and update the stable project handle.
    ///
    /// # Panics
    ///
    /// Panics if the settings mutex is poisoned.
    pub fn apply_project_settings(&mut self, settings: Settings) {
        if let Some(project) = self.project() {
            project.reload_from_settings(self, &settings);
        }

        *self.settings.lock().unwrap() = settings;
    }
}

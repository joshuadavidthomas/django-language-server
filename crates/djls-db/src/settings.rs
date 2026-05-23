use djls_conf::Settings;
use djls_semantic::Db as SemanticDb;
use salsa::Setter;

use crate::db::DjangoDatabase;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SettingsUpdate {
    pub env_changed: bool,
    pub diagnostics_changed: bool,
}

impl DjangoDatabase {
    /// Get a clone of the current settings.
    ///
    /// # Panics
    ///
    /// Panics if the settings mutex is poisoned.
    pub fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    /// Store new settings and report which startup-sensitive settings changed.
    ///
    /// Project Facts are updated by the Django Discovery Run, not by mutating
    /// the old semantic Project input from this settings boundary.
    ///
    /// # Panics
    ///
    /// Panics if the settings mutex is poisoned (another thread panicked while holding the lock)
    pub fn set_settings(&mut self, settings: Settings) -> SettingsUpdate {
        let previous = self.settings();
        *self.settings.lock().unwrap() = settings;
        let current = self.settings();

        if previous.tagspecs() != current.tagspecs() {
            let revision = self.semantic_settings_revision();
            let next_revision = revision.revision(self) + 1;
            revision.set_revision(self).to(next_revision);
        }

        SettingsUpdate {
            env_changed: env_settings_changed(&previous, &current),
            diagnostics_changed: previous.diagnostics() != current.diagnostics(),
        }
    }
}

fn env_settings_changed(previous: &Settings, current: &Settings) -> bool {
    previous.venv_path() != current.venv_path()
        || previous.django_settings_module() != current.django_settings_module()
        || previous.pythonpath() != current.pythonpath()
        || previous.env_file() != current.env_file()
        || previous.django_environments() != current.django_environments()
}

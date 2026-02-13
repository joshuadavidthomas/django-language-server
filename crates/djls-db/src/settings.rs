use djls_conf::Settings;
use djls_project::Db as ProjectDb;
use djls_project::Interpreter;
use djls_project::load_env_file;
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

    /// Update the settings, updating the existing project's fields via manual
    /// comparison (Ruff/RA pattern) to avoid unnecessary Salsa invalidation.
    ///
    /// When a project exists, delegates to [`update_project_from_settings`] to
    /// surgically update only the fields that changed, keeping project identity
    /// stable. When no project exists, the settings are stored for future use.
    ///
    /// # Panics
    ///
    /// Panics if the settings mutex is poisoned (another thread panicked while holding the lock)
    pub fn set_settings(&mut self, settings: Settings) -> SettingsUpdate {
        let previous = self.settings();
        *self.settings.lock().unwrap() = settings;

        let diagnostics_changed = previous.diagnostics() != self.settings().diagnostics();

        if self.project().is_some() {
            let settings = self.settings();
            let env_changed = self.update_project_from_settings(&settings);
            return SettingsUpdate {
                env_changed,
                diagnostics_changed,
            };
        }

        SettingsUpdate {
            env_changed: false,
            diagnostics_changed,
        }
    }

    /// Update an existing project's fields from new settings, only calling
    /// Salsa setters when values actually change (Ruff/RA pattern).
    ///
    /// Returns `true` if environment-related fields changed (`interpreter`,
    /// `django_settings_module`, `pythonpath`), indicating the inspector should
    /// be refreshed.
    pub fn update_project_from_settings(&mut self, settings: &Settings) -> bool {
        let Some(project) = self.project() else {
            return false;
        };

        let mut env_changed = false;

        let new_interpreter = Interpreter::discover(settings.venv_path());
        if project.interpreter(self) != &new_interpreter {
            project.set_interpreter(self).to(new_interpreter);
            env_changed = true;
        }

        let new_dsm = settings
            .django_settings_module()
            .map(String::from)
            .or_else(|| {
                std::env::var("DJANGO_SETTINGS_MODULE")
                    .ok()
                    .filter(|s| !s.is_empty())
            });
        if project.django_settings_module(self) != &new_dsm {
            project.set_django_settings_module(self).to(new_dsm);
            env_changed = true;
        }

        let new_pythonpath = settings.pythonpath().to_vec();
        if project.pythonpath(self) != &new_pythonpath {
            project.set_pythonpath(self).to(new_pythonpath);
            env_changed = true;
        }

        // Re-parse the env file when settings change. The env_file path may
        // have changed, or the file contents may differ after a reload.
        let root = project.root(self).clone();
        let new_env_vars = load_env_file(&root, settings);
        if project.env_vars(self) != &new_env_vars {
            project.set_env_vars(self).to(new_env_vars);
            env_changed = true;
        }

        let new_tagspecs = settings.tagspecs().clone();
        if project.tagspecs(self) != &new_tagspecs {
            project.set_tagspecs(self).to(new_tagspecs);
        }

        env_changed
    }
}

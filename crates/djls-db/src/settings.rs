use djls_conf::Settings;
use djls_project::Db as ProjectDb;
use djls_project::ProjectInputData;
use salsa::Setter;

use crate::db::DjangoDatabase;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SettingsUpdate {
    pub env_changed: bool,
    pub diagnostics_changed: bool,
    pub semantic_changed: bool,
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
    /// When a project exists, loads project input data from the settings and
    /// delegates to `apply_project_inputs` to surgically update only the fields
    /// that changed, keeping project identity stable. When no project exists,
    /// the settings are stored for future use.
    ///
    /// # Panics
    ///
    /// Panics if the settings mutex is poisoned (another thread panicked while holding the lock)
    pub fn set_settings(&mut self, settings: Settings) -> SettingsUpdate {
        let project_inputs = self.project().map(|project| {
            let root = project.root(self).clone();
            ProjectInputData::load(self, &root, &settings)
        });

        self.apply_settings(settings, project_inputs)
    }

    /// Store fully loaded settings and update the existing project's fields
    /// from precomputed input data.
    ///
    /// Callers that hold broader application locks should use this instead of
    /// `set_settings`: all filesystem probing and config loading can complete
    /// before taking the lock, leaving only cheap comparisons and Salsa setters
    /// here.
    pub fn apply_loaded_settings(
        &mut self,
        settings: Settings,
        project_inputs: ProjectInputData,
    ) -> SettingsUpdate {
        self.apply_settings(settings, Some(project_inputs))
    }

    fn apply_settings(
        &mut self,
        settings: Settings,
        project_inputs: Option<ProjectInputData>,
    ) -> SettingsUpdate {
        let previous = self.settings();
        let diagnostics_changed = previous.diagnostics() != settings.diagnostics();
        let semantic_changed = previous.tagspecs() != settings.tagspecs();
        *self.settings.lock().unwrap() = settings;

        let env_changed = project_inputs.is_some_and(|inputs| self.apply_project_inputs(inputs));

        SettingsUpdate {
            env_changed,
            diagnostics_changed,
            semantic_changed,
        }
    }

    /// Update an existing project's fields from already loaded project input
    /// data, only calling Salsa setters when values actually change (Ruff/RA
    /// pattern).
    ///
    /// Returns `true` if environment-related fields changed (`interpreter`,
    /// `django_settings_module`, `pythonpath`, `env_vars`, `search_paths`),
    /// indicating project data should be refreshed by the caller.
    pub(crate) fn apply_project_inputs(&mut self, inputs: ProjectInputData) -> bool {
        let Some(project) = self.project() else {
            return false;
        };

        let mut env_changed = false;
        let ProjectInputData {
            search_paths,
            interpreter,
            django_settings_module,
            pythonpath,
            env_vars,
            tagspecs,
        } = inputs;

        if project.search_paths(self) != &search_paths {
            search_paths.register_roots(self);
            project.set_search_paths(self).to(search_paths);
            env_changed = true;
        }

        if project.interpreter(self) != &interpreter {
            project.set_interpreter(self).to(interpreter);
            env_changed = true;
        }

        if project.django_settings_module(self) != &django_settings_module {
            project
                .set_django_settings_module(self)
                .to(django_settings_module);
            env_changed = true;
        }

        if project.pythonpath(self) != &pythonpath {
            project.set_pythonpath(self).to(pythonpath);
            env_changed = true;
        }

        if project.env_vars(self) != &env_vars {
            project.set_env_vars(self).to(env_vars);
            env_changed = true;
        }

        if project.tagspecs(self) != &tagspecs {
            project.set_tagspecs(self).to(tagspecs);
        }

        env_changed
    }
}

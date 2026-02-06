//! Concrete Salsa database implementation for the Django Language Server.
//!
//! This module provides the concrete [`DjangoDatabase`] that implements all
//! the database traits from workspace, template, and project crates. This follows
//! Ruff's architecture pattern where the concrete database lives at the top level.

use std::sync::Arc;
use std::sync::Mutex;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::Settings;
use djls_project::template_dirs;
use djls_project::Db as ProjectDb;
use djls_project::Inspector;
use djls_project::Interpreter;
use djls_project::Project;
use djls_project::TemplateTags;
use djls_semantic::Db as SemanticDb;
use djls_semantic::TagIndex;
use djls_semantic::TagSpecs;
use djls_source::Db as SourceDb;
use djls_source::File;
use djls_source::FxDashMap;
use djls_templates::Db as TemplateDb;
use djls_workspace::Db as WorkspaceDb;
use djls_workspace::FileSystem;
use salsa::Setter;

/// Concrete Salsa database for the Django Language Server.
///
/// This database implements all the traits from various crates:
/// - [`WorkspaceDb`] for file system access and core operations
/// - [`TemplateDb`] for template parsing and diagnostics
/// - [`ProjectDb`] for project metadata and Python environment
#[salsa::db]
#[derive(Clone)]
pub struct DjangoDatabase {
    /// File system for reading file content (checks buffers first, then disk).
    fs: Arc<dyn FileSystem>,

    /// Registry of tracked files used by the workspace layer.
    files: Arc<FxDashMap<Utf8PathBuf, File>>,

    /// The single project for this database instance
    project: Arc<Mutex<Option<Project>>>,

    /// Configuration settings for the language server
    settings: Arc<Mutex<Settings>>,

    /// Shared inspector for executing Python queries
    inspector: Arc<Inspector>,

    storage: salsa::Storage<Self>,

    // The logs are only used for testing and demonstrating reuse:
    #[cfg(test)]
    #[allow(dead_code)]
    logs: Arc<Mutex<Option<Vec<String>>>>,
}

#[cfg(test)]
impl Default for DjangoDatabase {
    fn default() -> Self {
        use djls_workspace::InMemoryFileSystem;

        let logs = <Arc<Mutex<Option<Vec<String>>>>>::default();

        Self {
            fs: Arc::new(InMemoryFileSystem::new()),
            files: Arc::new(FxDashMap::default()),
            project: Arc::new(Mutex::new(None)),
            settings: Arc::new(Mutex::new(Settings::default())),
            inspector: Arc::new(Inspector::new()),
            storage: salsa::Storage::new(Some(Box::new({
                let logs = logs.clone();
                move |event| {
                    eprintln!("Event: {event:?}");
                    // Log interesting events, if logging is enabled
                    if let Some(logs) = &mut *logs.lock().unwrap() {
                        // only log interesting events
                        if let salsa::EventKind::WillExecute { .. } = event.kind {
                            logs.push(format!("Event: {event:?}"));
                        }
                    }
                }
            }))),
            logs,
        }
    }
}

impl DjangoDatabase {
    /// Create a new [`DjangoDatabase`] with the given file system handle.
    pub fn new(
        file_system: Arc<dyn FileSystem>,
        settings: &Settings,
        project_path: Option<&Utf8Path>,
    ) -> Self {
        let mut db = Self {
            fs: file_system,
            files: Arc::new(FxDashMap::default()),
            project: Arc::new(Mutex::new(None)),
            settings: Arc::new(Mutex::new(settings.clone())),
            inspector: Arc::new(Inspector::new()),
            storage: salsa::Storage::new(None),
            #[cfg(test)]
            logs: Arc::new(Mutex::new(None)),
        };

        if let Some(path) = project_path {
            db.set_project(path, settings);
        }

        db
    }

    fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    /// Update the settings and propagate changes to the Project.
    ///
    /// This method delegates to `update_project_from_settings()` to ensure
    /// manual comparison is performed before calling Salsa setters.
    ///
    /// # Panics
    ///
    /// Panics if the settings mutex is poisoned (another thread panicked while holding the lock)
    pub fn set_settings(&mut self, settings: &Settings) {
        // Store new settings in mutex (still needed for non-Project uses)
        *self.settings.lock().unwrap() = settings.clone();

        if let Some(project) = self.project() {
            // Update Project fields with comparison
            self.update_project_from_settings(project, settings);
        }
    }

    /// Initialize or update the project.
    ///
    /// If no project exists, creates one. If project exists, updates
    /// fields via setters ONLY when values actually changed (Ruff/RA style).
    fn set_project(&mut self, root: &Utf8Path, settings: &Settings) {
        if self.project().is_some() {
            // Project exists - update via setters with manual comparison
            if let Some(project) = self.project() {
                self.update_project_from_settings(project, settings);
            }
        } else {
            // No project yet - create one
            let project = Project::bootstrap(
                self,
                root,
                settings.venv_path(),
                settings.django_settings_module(),
                settings.pythonpath(),
                settings,
            );
            *self.project.lock().unwrap() = Some(project);

            // Refresh inspector after project creation
            self.refresh_inspector();
        }
    }

    /// Update Project fields from settings, comparing before setting.
    ///
    /// Only calls Salsa setters when values actually changed.
    /// This is the Ruff/RA pattern to avoid unnecessary invalidation.
    fn update_project_from_settings(&mut self, project: Project, settings: &Settings) {
        let mut env_changed = false;

        // Check and update interpreter
        let new_interpreter = Interpreter::discover(settings.venv_path());
        if project.interpreter(self) != &new_interpreter {
            project.set_interpreter(self).to(new_interpreter);
            env_changed = true;
        }

        // Check and update django_settings_module
        let new_dsm = settings.django_settings_module().map(String::from);
        if project.django_settings_module(self) != &new_dsm {
            project.set_django_settings_module(self).to(new_dsm);
            env_changed = true;
        }

        // Check and update pythonpath
        let new_pp = settings.pythonpath().to_vec();
        if project.pythonpath(self) != &new_pp {
            project.set_pythonpath(self).to(new_pp);
            env_changed = true;
        }

        // Check and update tagspecs (config doc, not TagSpecs!)
        let new_tagspecs = settings.tagspecs().clone();
        if project.tagspecs(self) != &new_tagspecs {
            tracing::debug!("Tagspecs config changed, updating Project");
            project.set_tagspecs(self).to(new_tagspecs);
        }

        // Check and update diagnostics
        let new_diagnostics = settings.diagnostics().clone();
        if project.diagnostics(self) != &new_diagnostics {
            tracing::debug!("Diagnostics config changed, updating Project");
            project.set_diagnostics(self).to(new_diagnostics);
        }

        // Refresh inspector if environment changed
        if env_changed {
            tracing::debug!("Python environment changed, refreshing inspector");
            self.refresh_inspector();
        }
    }

    /// Refresh the inspector inventory by querying Python.
    ///
    /// This method:
    /// 1. Queries the Python inspector via the tracked `templatetags` function
    /// 2. Compares new inventory with current
    /// 3. Updates `Project.inspector_inventory` ONLY if changed
    ///
    /// Call this when:
    /// - Project is first initialized
    /// - Python environment changes (venv, PYTHONPATH)
    /// - User explicitly requests refresh (e.g., after pip install)
    pub fn refresh_inspector(&mut self) {
        use djls_project::templatetags;

        let Some(project) = self.project() else {
            tracing::warn!("Cannot refresh inspector: no project set");
            return;
        };

        // Query templatetags through the tracked function
        let new_inventory = templatetags(self, project);

        // Compare before setting (Ruff/RA style)
        let current = project.inspector_inventory(self);
        if current == &new_inventory {
            tracing::trace!("Inspector inventory unchanged, skipping update");
        } else {
            tracing::debug!(
                "Inspector inventory changed: {} -> {} tags",
                current.as_ref().map_or(0, TemplateTags::len),
                new_inventory.as_ref().map_or(0, TemplateTags::len)
            );
            project.set_inspector_inventory(self).to(new_inventory);
        }
    }
}

#[salsa::db]
impl salsa::Database for DjangoDatabase {}

#[salsa::db]
impl SourceDb for DjangoDatabase {
    fn create_file(&self, path: &Utf8Path) -> File {
        let file = File::new(self, path.to_owned(), 0);
        self.files.insert(path.to_owned(), file);
        file
    }

    fn get_file(&self, path: &Utf8Path) -> Option<File> {
        self.files.get(path).map(|entry| *entry)
    }

    fn read_file(&self, path: &Utf8Path) -> std::io::Result<String> {
        self.fs.read_to_string(path)
    }
}

#[salsa::db]
impl WorkspaceDb for DjangoDatabase {
    fn fs(&self) -> Arc<dyn FileSystem> {
        self.fs.clone()
    }
}

#[salsa::db]
impl TemplateDb for DjangoDatabase {}

#[salsa::db]
impl SemanticDb for DjangoDatabase {
    fn tag_specs(&self) -> TagSpecs {
        TagSpecs::from(&self.settings())
    }

    fn tag_index(&self) -> TagIndex<'_> {
        TagIndex::from_specs(self)
    }

    fn template_dirs(&self) -> Option<Vec<Utf8PathBuf>> {
        if let Some(project) = self.project() {
            template_dirs(self, project)
        } else {
            None
        }
    }

    fn diagnostics_config(&self) -> djls_conf::DiagnosticsConfig {
        self.settings().diagnostics().clone()
    }
}

#[salsa::db]
impl ProjectDb for DjangoDatabase {
    fn project(&self) -> Option<Project> {
        *self.project.lock().unwrap()
    }

    fn inspector(&self) -> Arc<Inspector> {
        self.inspector.clone()
    }
}

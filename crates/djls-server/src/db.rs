//! Concrete Salsa database implementation for the Django Language Server.
//!
//! This module provides the concrete [`DjangoDatabase`] that implements all
//! the database traits from workspace, template, and project crates. This follows
//! Ruff's architecture pattern where the concrete database lives at the top level.

use std::sync::Arc;
use std::sync::Mutex;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_project::Db as ProjectDb;
use djls_project::InspectorPool;
use djls_project::Interpreter;
use djls_project::Project;
use djls_semantic::Db as SemanticDb;
use djls_semantic::TagSpecs;
use djls_source::Db as SourceDb;
use djls_templates::db::Db as TemplateDb;
use djls_workspace::db::Db as WorkspaceDb;
use djls_workspace::FileSystem;

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

    /// The single project for this database instance
    project: Arc<Mutex<Option<Project>>>,

    /// Shared inspector pool for executing Python queries
    inspector_pool: Arc<InspectorPool>,

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
            project: Arc::new(Mutex::new(None)),
            inspector_pool: Arc::new(InspectorPool::new()),
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
    /// Set the project for this database instance
    ///
    /// # Panics
    ///
    /// Panics if the project mutex is poisoned.
    pub fn set_project(&self, root: &Utf8Path) {
        let interpreter = Interpreter::Auto;
        let django_settings = std::env::var("DJANGO_SETTINGS_MODULE").ok();

        let project = Project::new(self, root.to_path_buf(), interpreter, django_settings);

        *self.project.lock().unwrap() = Some(project);
    }
    /// Create a new [`DjangoDatabase`] with the given file system handle.
    pub fn new(file_system: Arc<dyn FileSystem>) -> Self {
        Self {
            fs: file_system,
            project: Arc::new(Mutex::new(None)),
            inspector_pool: Arc::new(InspectorPool::new()),
            storage: salsa::Storage::new(None),
            #[cfg(test)]
            logs: Arc::new(Mutex::new(None)),
        }
    }
}

#[salsa::db]
impl salsa::Database for DjangoDatabase {}

#[salsa::db]
impl SourceDb for DjangoDatabase {
    fn read_file_source(&self, path: &Utf8Path) -> std::io::Result<String> {
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
    fn tag_specs(&self) -> Arc<TagSpecs> {
        let project_root = if let Some(project) = self.project() {
            project.root(self).clone()
        } else {
            std::env::current_dir()
                .ok()
                .and_then(|p| Utf8PathBuf::from_path_buf(p).ok())
                .unwrap_or_else(|| Utf8PathBuf::from("."))
        };

        let tag_specs = if let Ok(settings) = djls_conf::Settings::new(&project_root) {
            TagSpecs::from(&settings)
        } else {
            djls_semantic::django_builtin_specs()
        };

        Arc::new(tag_specs)
    }
}

#[salsa::db]
impl ProjectDb for DjangoDatabase {
    fn project(&self) -> Option<Project> {
        *self.project.lock().unwrap()
    }

    fn inspector_pool(&self) -> Arc<InspectorPool> {
        self.inspector_pool.clone()
    }
}

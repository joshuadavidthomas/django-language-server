//! Concrete Salsa database implementation for the Django Language Server.
//!
//! This module provides the concrete [`DjangoDatabase`] that implements all
//! the database traits from workspace, template, and project crates. This follows
//! Ruff's architecture pattern where the concrete database lives at the top level.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;

use dashmap::DashMap;
use djls_project::Db as ProjectDb;
use djls_project::ProjectMetadata;
use djls_templates::db::Db as TemplateDb;
use djls_templates::templatetags::TagSpecs;
use djls_workspace::db::Db as WorkspaceDb;
use djls_workspace::db::SourceFile;
use djls_workspace::FileKind;
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

    /// Maps paths to [`SourceFile`] entities for O(1) lookup.
    files: Arc<DashMap<PathBuf, SourceFile>>,

    /// Project metadata containing root path and venv configuration.
    metadata: ProjectMetadata,

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
            files: Arc::new(DashMap::new()),
            metadata: ProjectMetadata::new(PathBuf::from("/test"), None),
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
    /// Create a new [`DjangoDatabase`] with the given file system, file map, and project metadata.
    pub fn new(
        file_system: Arc<dyn FileSystem>,
        files: Arc<DashMap<PathBuf, SourceFile>>,
        metadata: ProjectMetadata,
    ) -> Self {
        Self {
            fs: file_system,
            files,
            metadata,
            storage: salsa::Storage::new(None),
            #[cfg(test)]
            logs: Arc::new(Mutex::new(None)),
        }
    }

    /// Get an existing [`SourceFile`] for the given path without creating it.
    ///
    /// Returns `Some(SourceFile)` if the file is already tracked, `None` otherwise.
    pub fn get_file(&self, path: &Path) -> Option<SourceFile> {
        self.files.get(path).map(|file_ref| *file_ref)
    }

    /// Get or create a [`SourceFile`] for the given path.
    ///
    /// Files are created with an initial revision of 0 and tracked in the database's
    /// `DashMap`. The `Arc` ensures cheap cloning while maintaining thread safety.
    pub fn get_or_create_file(&mut self, path: &PathBuf) -> SourceFile {
        if let Some(file_ref) = self.files.get(path) {
            return *file_ref;
        }

        // File doesn't exist, so we need to create it
        let kind = FileKind::from_path(path);
        let file = SourceFile::new(self, kind, Arc::from(path.to_string_lossy().as_ref()), 0);

        self.files.insert(path.clone(), file);
        file
    }

    /// Check if a file is being tracked without creating it.
    pub fn has_file(&self, path: &Path) -> bool {
        self.files.contains_key(path)
    }

    /// Touch a file to mark it as modified, triggering re-evaluation of dependent queries.
    ///
    /// Updates the file's revision number to signal that cached query results
    /// depending on this file should be invalidated.
    pub fn touch_file(&mut self, path: &Path) {
        let Some(file_ref) = self.files.get(path) else {
            tracing::debug!("File {} not tracked, skipping touch", path.display());
            return;
        };
        let file = *file_ref;
        drop(file_ref); // Explicitly drop to release the lock

        let current_rev = file.revision(self);
        let new_rev = current_rev + 1;
        file.set_revision(self).to(new_rev);

        tracing::debug!(
            "Touched {}: revision {} -> {}",
            path.display(),
            current_rev,
            new_rev
        );
    }
}

#[salsa::db]
impl salsa::Database for DjangoDatabase {}

#[salsa::db]
impl WorkspaceDb for DjangoDatabase {
    fn fs(&self) -> Arc<dyn FileSystem> {
        self.fs.clone()
    }

    fn read_file_content(&self, path: &Path) -> std::io::Result<String> {
        self.fs.read_to_string(path)
    }
}

#[salsa::db]
impl TemplateDb for DjangoDatabase {
    fn tag_specs(&self) -> Arc<TagSpecs> {
        let project_root = self.metadata.root();

        if let Ok(user_specs) = TagSpecs::load_user_specs(project_root) {
            // If user specs exist and aren't empty, merge with built-in specs
            // to allow user overrides while keeping built-in specs as fallback
            if let Ok(mut builtin_specs) = TagSpecs::load_builtin_specs() {
                builtin_specs.merge(user_specs);
                return Arc::new(builtin_specs);
            }
            return Arc::new(user_specs);
        }

        // Fall back to built-in specs
        Arc::new(TagSpecs::load_builtin_specs().expect("Built-in specs must be valid"))
    }
}

#[salsa::db]
impl ProjectDb for DjangoDatabase {
    fn metadata(&self) -> &ProjectMetadata {
        &self.metadata
    }
}

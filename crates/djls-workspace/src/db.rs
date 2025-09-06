//! Base database trait for workspace operations.
//!
//! This module provides the base [`Db`] trait that defines file system access
//! and core file tracking functionality. The concrete database implementation
//! lives in the server crate, following Ruff's architecture pattern.
//!
//! ## Architecture
//!
//! The system uses a layered trait approach:
//! 1. **Base trait** ([`Db`]) - Defines file system access methods (this module)
//! 2. **Extension traits** - Other crates (like djls-templates) extend this trait
//! 3. **Concrete implementation** - Server crate implements all traits
//!
//! ## The Revision Dependency
//!
//! The [`source_text`] function **must** call `file.revision(db)` to create
//! a Salsa dependency. Without this, revision changes won't invalidate queries:
//!
//! ```ignore
//! let _ = file.revision(db);  // Creates the dependency chain!
//! ```

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;

use dashmap::DashMap;
use salsa::Setter;

use crate::FileKind;
use crate::FileSystem;

/// Base database trait that provides file system access for Salsa queries
#[salsa::db]
pub trait Db: salsa::Database {
    /// Get the file system for reading files.
    fn fs(&self) -> Arc<dyn FileSystem>;

    /// Read file content through the file system.
    ///
    /// Checks buffers first via [`WorkspaceFileSystem`](crate::fs::WorkspaceFileSystem),
    /// then falls back to disk.
    fn read_file_content(&self, path: &Path) -> std::io::Result<String>;
}

/// Temporary concrete database for workspace.
///
/// This will be moved to the server crate in the refactoring.
/// For now, it's kept here to avoid breaking existing code.
#[salsa::db]
#[derive(Clone)]
pub struct Database {
    storage: salsa::Storage<Self>,

    /// File system for reading file content (checks buffers first, then disk).
    fs: Arc<dyn FileSystem>,

    /// Maps paths to [`SourceFile`] entities for O(1) lookup.
    files: Arc<DashMap<PathBuf, SourceFile>>,

    // The logs are only used for testing and demonstrating reuse:
    #[cfg(test)]
    #[allow(dead_code)]
    logs: Arc<Mutex<Option<Vec<String>>>>,
}

#[cfg(test)]
impl Default for Database {
    fn default() -> Self {
        use crate::fs::InMemoryFileSystem;

        let logs = <Arc<Mutex<Option<Vec<String>>>>>::default();
        Self {
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
            fs: Arc::new(InMemoryFileSystem::new()),
            files: Arc::new(DashMap::new()),
            logs,
        }
    }
}

impl Database {
    pub fn new(file_system: Arc<dyn FileSystem>, files: Arc<DashMap<PathBuf, SourceFile>>) -> Self {
        Self {
            storage: salsa::Storage::new(None),
            fs: file_system,
            files,
            #[cfg(test)]
            logs: Arc::new(Mutex::new(None)),
        }
    }

    /// Read file content through the file system.
    pub fn read_file_content(&self, path: &Path) -> std::io::Result<String> {
        self.fs.read_to_string(path)
    }

    /// Get an existing [`SourceFile`] for the given path without creating it.
    ///
    /// Returns `Some(SourceFile)` if the file is already tracked, `None` otherwise.
    /// This method uses an immutable reference and doesn't modify the database.
    pub fn get_file(&self, path: &Path) -> Option<SourceFile> {
        self.files.get(path).map(|file_ref| *file_ref)
    }

    /// Get or create a [`SourceFile`] for the given path.
    ///
    /// Files are created with an initial revision of 0 and tracked in the [`Database`]'s
    /// `DashMap`. The `Arc` ensures cheap cloning while maintaining thread safety.
    ///
    /// ## Thread Safety
    ///
    /// This method is inherently thread-safe despite the check-then-create pattern because
    /// it requires `&mut self`, ensuring exclusive access to the Database. Only one thread
    /// can call this method at a time due to Rust's ownership rules.
    pub fn get_or_create_file(&mut self, path: &PathBuf) -> SourceFile {
        if let Some(file_ref) = self.files.get(path) {
            // Copy the value (SourceFile is Copy)
            // The guard drops automatically, no need for explicit drop
            return *file_ref;
        }

        // File doesn't exist, so we need to create it
        let kind = FileKind::from_path(path);
        let file = SourceFile::new(self, kind, Arc::from(path.to_string_lossy().as_ref()), 0);

        self.files.insert(path.clone(), file);
        file
    }

    /// Check if a file is being tracked without creating it.
    ///
    /// This is primarily used for testing to verify that files have been
    /// created without affecting the database state.
    pub fn has_file(&self, path: &Path) -> bool {
        self.files.contains_key(path)
    }

    /// Touch a file to mark it as modified, triggering re-evaluation of dependent queries.
    ///
    /// Similar to Unix `touch`, this updates the file's revision number to signal
    /// that cached query results depending on this file should be invalidated.
    ///
    /// This is typically called when:
    /// - A file is opened in the editor (if it was previously cached from disk)
    /// - A file's content is modified
    /// - A file's buffer is closed (reverting to disk content)
    pub fn touch_file(&mut self, path: &Path) {
        // Get the file if it exists
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
impl salsa::Database for Database {}

#[salsa::db]
impl Db for Database {
    fn fs(&self) -> Arc<dyn FileSystem> {
        self.fs.clone()
    }

    fn read_file_content(&self, path: &Path) -> std::io::Result<String> {
        self.fs.read_to_string(path)
    }
}

/// Represents a single file without storing its content.
///
/// [`SourceFile`] is a Salsa input entity that tracks a file's path, revision, and
/// classification for analysis routing. Following Ruff's pattern, content is NOT
/// stored here but read on-demand through the `source_text` tracked function.
#[salsa::input]
pub struct SourceFile {
    /// The file's classification for analysis routing
    pub kind: FileKind,
    /// The file path
    #[returns(ref)]
    pub path: Arc<str>,
    /// The revision number for invalidation tracking
    pub revision: u64,
}

/// Read file content, creating a Salsa dependency on the file's revision.
#[salsa::tracked]
pub fn source_text(db: &dyn Db, file: SourceFile) -> Arc<str> {
    // This line creates the Salsa dependency on revision! Without this call,
    // revision changes won't trigger invalidation
    let _ = file.revision(db);

    let path = Path::new(file.path(db).as_ref());
    match db.read_file_content(path) {
        Ok(content) => Arc::from(content),
        Err(_) => {
            Arc::from("") // Return empty string for missing files
        }
    }
}

/// Represents a file path for Salsa tracking.
///
/// [`FilePath`] is a Salsa input entity that tracks a file path for use in
/// path-based queries. This allows Salsa to properly track dependencies
/// on files identified by path rather than by SourceFile input.
#[salsa::input]
pub struct FilePath {
    /// The file path as a string
    #[returns(ref)]
    pub path: Arc<str>,
}

// Template-specific functionality has been moved to djls-templates crate
// See djls_templates::db for template parsing and diagnostics

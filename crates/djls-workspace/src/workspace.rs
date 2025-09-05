//! Workspace facade for managing all workspace components
//!
//! This module provides the [`Workspace`] struct that encapsulates all workspace
//! components including buffers, file system, file tracking, and database handle.
//! This provides a clean API boundary between server and workspace layers.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use dashmap::DashMap;
use tower_lsp_server::lsp_types::TextDocumentContentChangeEvent;
use url::Url;

use crate::buffers::Buffers;
use crate::db::Database;
use crate::db::SourceFile;
use crate::document::TextDocument;
use crate::fs::OsFileSystem;
use crate::fs::WorkspaceFileSystem;
use crate::paths::url_to_path;
use crate::storage::SafeStorageHandle;

/// Workspace facade that encapsulates all workspace components.
///
/// This struct provides a unified interface for managing workspace state,
/// including in-memory buffers, file system abstraction, file tracking,
/// and the Salsa database handle.
pub struct Workspace {
    /// Thread-safe shared buffer storage for open documents
    buffers: Buffers,
    /// File system abstraction with buffer interception
    file_system: Arc<WorkspaceFileSystem>,
    /// Shared file tracking across all Database instances
    files: Arc<DashMap<PathBuf, SourceFile>>,
    /// Thread-safe Salsa database handle for incremental computation with safe mutation management
    db_handle: SafeStorageHandle,
}

impl Workspace {
    /// Create a new [`Workspace`] with all components initialized.
    #[must_use]
    pub fn new() -> Self {
        let buffers = Buffers::new();
        let files = Arc::new(DashMap::new());
        let file_system = Arc::new(WorkspaceFileSystem::new(
            buffers.clone(),
            Arc::new(OsFileSystem),
        ));
        let handle = Database::new(file_system.clone(), files.clone())
            .storage()
            .clone()
            .into_zalsa_handle();

        Self {
            buffers,
            file_system,
            files,
            db_handle: SafeStorageHandle::new(handle),
        }
    }

    /// Execute a read-only operation with access to the database.
    ///
    /// Creates a temporary Database instance from the handle for the closure.
    /// This is safe for concurrent read operations.
    pub fn with_db<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Database) -> R,
    {
        let handle = self.db_handle.clone_for_read();
        let storage = handle.into_storage();
        let db = Database::from_storage(storage, self.file_system.clone(), self.files.clone());
        f(&db)
    }

    /// Execute a mutable operation with exclusive access to the database.
    ///
    /// Uses the `StorageHandleGuard` pattern to ensure the handle is safely restored
    /// even if the operation panics. This eliminates the need for placeholder handles.
    pub fn with_db_mut<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut Database) -> R,
    {
        let mut guard = self.db_handle.take_guarded();
        let handle = guard.handle();
        let storage = handle.into_storage();
        let mut db = Database::from_storage(storage, self.file_system.clone(), self.files.clone());
        let result = f(&mut db);
        let new_handle = db.storage().clone().into_zalsa_handle();
        guard.restore(new_handle);
        result
    }

    /// Open a document in the workspace.
    ///
    /// Updates both the buffer layer and database layer. Creates the file in
    /// the database or invalidates it if it already exists.
    pub fn open_document(&mut self, url: &Url, document: TextDocument) {
        // Layer 1: Add to buffers
        self.buffers.open(url.clone(), document);

        // Layer 2: Create file and touch if it already exists
        if let Some(path) = url_to_path(url) {
            self.with_db_mut(|db| {
                // Check if file already exists (was previously read from disk)
                let already_exists = db.has_file(&path);
                let _file = db.get_or_create_file(&path);

                if already_exists {
                    // File was already read - touch to invalidate cache
                    db.touch_file(&path);
                }
                // Note: New files automatically start at revision 0, no additional action needed
            });
        }
    }

    /// Update a document with incremental changes.
    ///
    /// Applies changes to the existing document and triggers database invalidation.
    /// Falls back to full replacement if the document isn't currently open.
    pub fn update_document(
        &mut self,
        url: &Url,
        changes: Vec<TextDocumentContentChangeEvent>,
        version: i32,
        encoding: crate::encoding::PositionEncoding,
    ) {
        if let Some(mut document) = self.buffers.get(url) {
            // Apply incremental changes to existing document
            document.update(changes, version, encoding);
            self.buffers.update(url.clone(), document);
        } else if let Some(first_change) = changes.into_iter().next() {
            // Fallback: treat first change as full replacement
            if first_change.range.is_none() {
                let document = TextDocument::new(
                    first_change.text,
                    version,
                    crate::language::LanguageId::Other,
                );
                self.buffers.open(url.clone(), document);
            }
        }

        // Touch file in database to trigger invalidation
        if let Some(path) = url_to_path(url) {
            self.invalidate_file_if_exists(&path);
        }
    }

    /// Close a document and return it.
    ///
    /// Removes from buffers and triggers database invalidation to fall back to disk.
    pub fn close_document(&mut self, url: &Url) -> Option<TextDocument> {
        let document = self.buffers.close(url);

        // Touch file in database to trigger re-read from disk
        if let Some(path) = url_to_path(url) {
            self.invalidate_file_if_exists(&path);
        }

        document
    }

    /// Get a document from the buffer if it's open.
    ///
    /// Returns a cloned [`TextDocument`] for the given URL if it exists in buffers.
    #[must_use]
    pub fn get_document(&self, url: &Url) -> Option<TextDocument> {
        self.buffers.get(url)
    }

    /// Invalidate a file if it exists in the database.
    ///
    /// Used by document lifecycle methods to trigger cache invalidation.
    fn invalidate_file_if_exists(&mut self, path: &Path) {
        self.with_db_mut(|db| {
            if db.has_file(path) {
                db.touch_file(path);
            }
        });
    }

    #[must_use]
    pub fn db_handle(&self) -> &SafeStorageHandle {
        &self.db_handle
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use tempfile::tempdir;

    use super::*;
    use crate::db::source_text;
    use crate::encoding::PositionEncoding;
    use crate::LanguageId;

    #[test]
    fn test_with_db_read() {
        // Read-only access works
        let workspace = Workspace::new();

        let result = workspace.with_db(|db| {
            // Can perform read operations
            db.has_file(&PathBuf::from("test.py"))
        });

        assert!(!result); // File doesn't exist yet
    }

    #[test]
    fn test_with_db_mut() {
        // Mutation access works
        let mut workspace = Workspace::new();

        // Create a file through mutation
        workspace.with_db_mut(|db| {
            let path = PathBuf::from("test.py");
            let _file = db.get_or_create_file(&path);
        });

        // Verify it exists
        let exists = workspace.with_db(|db| db.has_file(&PathBuf::from("test.py")));
        assert!(exists);
    }

    #[test]
    fn test_concurrent_reads() {
        // Multiple with_db calls can run simultaneously
        let workspace = Arc::new(Workspace::new());

        let w1 = workspace.clone();
        let w2 = workspace.clone();

        // Spawn concurrent reads
        let handle1 =
            std::thread::spawn(move || w1.with_db(|db| db.has_file(&PathBuf::from("file1.py"))));

        let handle2 =
            std::thread::spawn(move || w2.with_db(|db| db.has_file(&PathBuf::from("file2.py"))));

        // Both should complete without issues
        assert!(!handle1.join().unwrap());
        assert!(!handle2.join().unwrap());
    }

    #[test]
    fn test_sequential_mutations() {
        // Multiple with_db_mut calls work in sequence
        let mut workspace = Workspace::new();

        // First mutation
        workspace.with_db_mut(|db| {
            let _file = db.get_or_create_file(&PathBuf::from("first.py"));
        });

        // Second mutation
        workspace.with_db_mut(|db| {
            let _file = db.get_or_create_file(&PathBuf::from("second.py"));
        });

        // Both files should exist
        let (has_first, has_second) = workspace.with_db(|db| {
            (
                db.has_file(&PathBuf::from("first.py")),
                db.has_file(&PathBuf::from("second.py")),
            )
        });

        assert!(has_first);
        assert!(has_second);
    }

    #[test]
    fn test_open_document() {
        // Open doc → appears in buffers → queryable via db
        let mut workspace = Workspace::new();
        let url = Url::parse("file:///test.py").unwrap();

        // Open document
        let document = TextDocument::new("print('hello')".to_string(), 1, LanguageId::Python);
        workspace.open_document(&url, document);

        // Should be in buffers
        assert!(workspace.buffers.get(&url).is_some());

        // Should be queryable through database
        let content = workspace.with_db_mut(|db| {
            let file = db.get_or_create_file(&PathBuf::from("/test.py"));
            source_text(db, file).to_string()
        });

        assert_eq!(content, "print('hello')");
    }

    #[test]
    fn test_update_document() {
        // Update changes buffer content
        let mut workspace = Workspace::new();
        let url = Url::parse("file:///test.py").unwrap();

        // Open with initial content
        let document = TextDocument::new("initial".to_string(), 1, LanguageId::Python);
        workspace.open_document(&url, document);

        // Update content
        let changes = vec![TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "updated".to_string(),
        }];
        workspace.update_document(&url, changes, 2, PositionEncoding::Utf16);

        // Verify buffer was updated
        let buffer = workspace.buffers.get(&url).unwrap();
        assert_eq!(buffer.content(), "updated");
        assert_eq!(buffer.version(), 2);
    }

    #[test]
    fn test_close_document() {
        // Close removes from buffers
        let mut workspace = Workspace::new();
        let url = Url::parse("file:///test.py").unwrap();

        // Open document
        let document = TextDocument::new("content".to_string(), 1, LanguageId::Python);
        workspace.open_document(&url, document.clone());

        // Close it
        let closed = workspace.close_document(&url);
        assert!(closed.is_some());

        // Should no longer be in buffers
        assert!(workspace.buffers.get(&url).is_none());
    }

    #[test]
    fn test_buffer_takes_precedence_over_disk() {
        // Open doc content overrides file system
        let temp_dir = tempdir().unwrap();
        let file_path = temp_dir.path().join("test.py");
        std::fs::write(&file_path, "disk content").unwrap();

        let mut workspace = Workspace::new();
        let url = Url::from_file_path(&file_path).unwrap();

        // Open document with different content than disk
        let document = TextDocument::new("buffer content".to_string(), 1, LanguageId::Python);
        workspace.open_document(&url, document);

        // Database should return buffer content, not disk content
        let content = workspace.with_db_mut(|db| {
            let file = db.get_or_create_file(&file_path);
            source_text(db, file).to_string()
        });

        assert_eq!(content, "buffer content");
    }

    #[test]
    fn test_missing_file_returns_empty() {
        // Non-existent files return "" not error
        let mut workspace = Workspace::new();

        let content = workspace.with_db_mut(|db| {
            let file = db.get_or_create_file(&PathBuf::from("/nonexistent.py"));
            source_text(db, file).to_string()
        });

        assert_eq!(content, "");
    }

    #[test]
    fn test_file_invalidation_on_touch() {
        // touch_file triggers Salsa recomputation
        let temp_dir = tempdir().unwrap();
        let file_path = temp_dir.path().join("test.py");
        std::fs::write(&file_path, "version 1").unwrap();

        let mut workspace = Workspace::new();

        // First read
        let content1 = workspace.with_db_mut(|db| {
            let file = db.get_or_create_file(&file_path);
            source_text(db, file).to_string()
        });
        assert_eq!(content1, "version 1");

        // Update file on disk
        std::fs::write(&file_path, "version 2").unwrap();

        // Touch to invalidate
        workspace.with_db_mut(|db| {
            db.touch_file(&file_path);
        });

        // Should read new content
        let content2 = workspace.with_db_mut(|db| {
            let file = db.get_or_create_file(&file_path);
            source_text(db, file).to_string()
        });
        assert_eq!(content2, "version 2");
    }
}

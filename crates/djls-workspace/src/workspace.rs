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
    ) {
        if let Some(mut document) = self.buffers.get(url) {
            // Apply incremental changes to existing document
            document.update(changes, version);
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

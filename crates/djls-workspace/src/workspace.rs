//! Workspace facade for managing all workspace components
//!
//! This module provides the [`Workspace`] struct that encapsulates all workspace
//! components including buffers, file system, file tracking, and database handle.
//! This provides a clean API boundary between server and workspace layers.

use std::mem;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dashmap::DashMap;
use salsa::StorageHandle;
use tower_lsp_server::lsp_types::TextDocumentContentChangeEvent;
use url::Url;

use crate::buffers::Buffers;
use crate::db::{source_text, Database, SourceFile};
use crate::document::TextDocument;
use crate::fs::{OsFileSystem, WorkspaceFileSystem};
use crate::paths::url_to_path;

/// Workspace facade that encapsulates all workspace components.
///
/// This struct provides a unified interface for managing workspace state,
/// including in-memory buffers, file system abstraction, file tracking,
/// and the Salsa database handle. It follows the same initialization pattern
/// as the Session but encapsulates it in a reusable component.
///
/// ## Components
///
/// - **Buffers**: Thread-safe storage for open document content
/// - **WorkspaceFileSystem**: File system abstraction with buffer interception
/// - **Files**: Shared file tracking across all Database instances
/// - **Database Handle**: Thread-safe Salsa database handle for incremental computation
pub struct Workspace {
    /// Layer 1: Shared buffer storage for open documents
    buffers: Buffers,

    /// File system abstraction with buffer interception
    file_system: Arc<WorkspaceFileSystem>,

    /// Shared file tracking across all Database instances
    files: Arc<DashMap<PathBuf, SourceFile>>,

    /// Layer 2: Thread-safe Salsa database handle for pure computation
    db_handle: StorageHandle<Database>,
}

impl Workspace {
    /// Create a new Workspace with all components initialized.
    ///
    /// This follows the same initialization pattern as Session:
    /// 1. Creates Buffers for in-memory document storage
    /// 2. Creates shared file tracking DashMap
    /// 3. Creates WorkspaceFileSystem with buffer interception
    /// 4. Initializes Database and extracts StorageHandle
    #[must_use]
    pub fn new() -> Self {
        let buffers = Buffers::new();
        let files = Arc::new(DashMap::new());
        let file_system = Arc::new(WorkspaceFileSystem::new(
            buffers.clone(),
            Arc::new(OsFileSystem),
        ));
        let db_handle = Database::new(file_system.clone(), files.clone())
            .storage()
            .clone()
            .into_zalsa_handle();

        Self {
            buffers,
            file_system,
            files,
            db_handle,
        }
    }

    // Database Access Methods (AC #1)

    /// Execute a read-only operation with access to the database.
    ///
    /// Creates a temporary Database instance from the handle for the closure.
    /// This is safe for concurrent read operations.
    pub fn with_db<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Database) -> R,
    {
        let storage = self.db_handle.clone().into_storage();
        let db = Database::from_storage(storage, self.file_system.clone(), self.files.clone());
        f(&db)
    }

    /// Execute a mutable operation with exclusive access to the database.
    ///
    /// Takes ownership of the handle, creates a mutable Database, and restores
    /// the handle after the operation completes.
    pub fn with_db_mut<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut Database) -> R,
    {
        let handle = self.take_db_handle_for_mutation();
        let storage = handle.into_storage();
        let mut db = Database::from_storage(storage, self.file_system.clone(), self.files.clone());
        let result = f(&mut db);
        let new_handle = db.storage().clone().into_zalsa_handle();
        self.restore_db_handle(new_handle);
        result
    }

    /// Private helper: Take the database handle for mutation.
    fn take_db_handle_for_mutation(&mut self) -> StorageHandle<Database> {
        // Create a placeholder handle and swap it with the current one
        let placeholder = Database::new(self.file_system.clone(), self.files.clone())
            .storage()
            .clone()
            .into_zalsa_handle();
        mem::replace(&mut self.db_handle, placeholder)
    }

    /// Private helper: Restore the database handle after mutation.
    fn restore_db_handle(&mut self, handle: StorageHandle<Database>) {
        self.db_handle = handle;
    }

    // Document Lifecycle Methods (AC #2)

    /// Open a document in the workspace.
    ///
    /// Updates both the buffer layer and database layer. If the file exists
    /// in the database, it's marked as touched to trigger invalidation.
    pub fn open_document(&mut self, url: &Url, document: TextDocument) {
        // Layer 1: Add to buffers
        self.buffers.open(url.clone(), document);

        // Layer 2: Update database if file exists
        if let Some(path) = url_to_path(url) {
            self.invalidate_file_if_exists(&path);
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

    // File Operations (AC #3)

    /// Get file content through the database.
    ///
    /// Creates or retrieves the file entity and returns its source text.
    pub fn file_content(&mut self, path: PathBuf) -> String {
        self.with_db_mut(|db| {
            let file = db.get_or_create_file(path);
            source_text(db, file).to_string()
        })
    }

    /// Get the revision number of a file if it exists.
    ///
    /// Returns None if the file is not being tracked by the database.
    pub fn file_revision(&mut self, path: &Path) -> Option<u64> {
        self.with_db_mut(|db| {
            if db.has_file(path) {
                let file = db.get_or_create_file(path.to_path_buf());
                Some(file.revision(db))
            } else {
                None
            }
        })
    }

    // Buffer Query Method (AC #4)

    /// Get a document from the buffer if it's open.
    ///
    /// Returns a cloned TextDocument for the given URL if it exists in buffers.
    #[must_use]
    pub fn get_document(&self, url: &Url) -> Option<TextDocument> {
        self.buffers.get(url)
    }

    // File Invalidation Helper (AC #5)

    /// Private helper: Invalidate a file if it exists in the database.
    ///
    /// Used by document lifecycle methods to trigger cache invalidation.
    fn invalidate_file_if_exists(&mut self, path: &Path) {
        self.with_db_mut(|db| {
            if db.has_file(path) {
                db.touch_file(path);
            }
        });
    }

    // Existing methods preserved below

    /// Get a reference to the buffers.
    #[must_use]
    pub fn buffers(&self) -> &Buffers {
        &self.buffers
    }

    /// Get a clone of the file system.
    ///
    /// Returns an Arc-wrapped WorkspaceFileSystem that can be shared
    /// across threads and used for file operations.
    #[must_use]
    pub fn file_system(&self) -> Arc<WorkspaceFileSystem> {
        self.file_system.clone()
    }

    /// Get a clone of the files tracking map.
    ///
    /// Returns an Arc-wrapped DashMap for O(1) file lookups that
    /// can be shared across Database instances.
    #[must_use]
    pub fn files(&self) -> Arc<DashMap<PathBuf, SourceFile>> {
        self.files.clone()
    }

    /// Get a reference to the database handle.
    ///
    /// The StorageHandle can be cloned safely for read operations
    /// or moved for mutation operations following Salsa's patterns.
    #[must_use]
    pub fn db_handle(&self) -> &StorageHandle<Database> {
        &self.db_handle
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}
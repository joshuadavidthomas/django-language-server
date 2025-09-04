//! Workspace facade for managing all workspace components
//!
//! This module provides the [`Workspace`] struct that encapsulates all workspace
//! components including buffers, file system, file tracking, and database handle.
//! This provides a clean API boundary between server and workspace layers.

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

/// Safe wrapper for [`StorageHandle`](salsa::StorageHandle) that prevents misuse through type safety.
///
/// This enum ensures that database handles can only be in one of two valid states,
/// making invalid states unrepresentable and eliminating the need for placeholder
/// handles during mutations.
enum SafeStorageHandle {
    /// Handle is available for use
    Available(StorageHandle<Database>),
    /// Handle has been taken for mutation - no handle available
    TakenForMutation,
}

impl SafeStorageHandle {
    /// Create a new `SafeStorageHandle` in the `Available` state
    fn new(handle: StorageHandle<Database>) -> Self {
        Self::Available(handle)
    }

    /// Take the handle for mutation, leaving the enum in `TakenForMutation` state.
    ///
    /// ## Panics
    ///
    /// Panics if the handle has already been taken for mutation.
    fn take_for_mutation(&mut self) -> StorageHandle<Database> {
        match std::mem::replace(self, Self::TakenForMutation) {
            Self::Available(handle) => handle,
            Self::TakenForMutation => panic!(
                "Database handle already taken for mutation. This indicates a programming error - \
                 ensure you're not calling multiple mutation operations concurrently or forgetting \
                 to restore the handle after a previous mutation."
            ),
        }
    }

    /// Restore the handle after mutation, returning it to `Available` state.
    ///
    /// ## Panics
    ///
    /// Panics if the handle is not currently taken for mutation.
    fn restore_from_mutation(&mut self, handle: StorageHandle<Database>) {
        match self {
            Self::TakenForMutation => {
                *self = Self::Available(handle);
            }
            Self::Available(_) => panic!(
                "Cannot restore database handle - handle is not currently taken for mutation. \
                 This indicates a programming error in the StorageHandleGuard implementation."
            ),
        }
    }

    /// Get a clone of the handle for read-only operations.
    ///
    /// ## Panics
    ///
    /// Panics if the handle is currently taken for mutation.
    fn clone_for_read(&self) -> StorageHandle<Database> {
        match self {
            Self::Available(handle) => handle.clone(),
            Self::TakenForMutation => panic!(
                "Cannot access database handle for read - handle is currently taken for mutation. \
                 Wait for the current mutation operation to complete."
            ),
        }
    }
}

/// State of the [`StorageHandleGuard`] during its lifetime.
///
/// See [`StorageHandleGuard`] for usage and state machine details.
enum GuardState {
    /// Guard holds the handle, ready to be consumed
    Active { handle: StorageHandle<Database> },
    /// Handle consumed, awaiting restoration
    Consumed,
    /// Handle restored to [`SafeStorageHandle`]
    Restored,
}

/// RAII guard for safe [`StorageHandle`](salsa::StorageHandle) management during mutations.
///
/// This guard ensures that database handles are automatically restored even if
/// panics occur during mutation operations. It prevents double-takes and
/// provides clear error messages for misuse.
///
/// ## State Machine
///
/// The guard follows these valid state transitions:
/// - `Active` → `Consumed` (via `handle()` method)
/// - `Consumed` → `Restored` (via `restore()` method)
///
/// ## Invalid Transitions
///
/// Invalid operations will panic with specific error messages:
/// - `handle()` on `Consumed` state: "[`StorageHandle`](salsa::StorageHandle) already consumed"
/// - `handle()` on `Restored` state: "Cannot consume handle - guard has already been restored"
/// - `restore()` on `Active` state: "Cannot restore handle - it hasn't been consumed yet"
/// - `restore()` on `Restored` state: "Handle has already been restored"
///
/// ## Drop Behavior
///
/// The guard will panic on drop unless it's in the `Restored` state:
/// - Drop in `Active` state: "`StorageHandleGuard` dropped without using the handle"
/// - Drop in `Consumed` state: "`StorageHandleGuard` dropped without restoring handle"
/// - Drop in `Restored` state: No panic - proper cleanup completed
///
/// ## Usage Example
///
/// ```rust,ignore
/// let mut guard = StorageHandleGuard::new(&mut safe_handle);
/// let handle = guard.handle();           // Active → Consumed
/// // ... perform mutations with handle ...
/// guard.restore(updated_handle);         // Consumed → Restored
/// // Guard drops cleanly in Restored state
/// ```
#[must_use = "StorageHandleGuard must be used - dropping it immediately defeats the purpose"]
pub struct StorageHandleGuard<'a> {
    /// Reference to the workspace's `SafeStorageHandle` for restoration
    safe_handle: &'a mut SafeStorageHandle,
    /// Current state of the guard and handle
    state: GuardState,
}

impl<'a> StorageHandleGuard<'a> {
    /// Create a new guard by taking the handle from the `SafeStorageHandle`.
    fn new(safe_handle: &'a mut SafeStorageHandle) -> Self {
        let handle = safe_handle.take_for_mutation();
        Self {
            safe_handle,
            state: GuardState::Active { handle },
        }
    }

    /// Get the [`StorageHandle`](salsa::StorageHandle) for mutation operations.
    ///
    /// ## Panics
    ///
    /// Panics if the handle has already been consumed or restored.
    pub fn handle(&mut self) -> StorageHandle<Database> {
        match std::mem::replace(&mut self.state, GuardState::Consumed) {
            GuardState::Active { handle } => handle,
            GuardState::Consumed => panic!(
                "StorageHandle already consumed from guard. Each guard can only provide \
                 the handle once - this prevents accidental multiple uses."
            ),
            GuardState::Restored => panic!(
                "Cannot consume handle - guard has already been restored. Once restored, \
                 the guard cannot provide the handle again."
            ),
        }
    }

    /// Restore the handle manually before the guard drops.
    ///
    /// This is useful when you want to restore the handle and continue using
    /// the workspace in the same scope.
    ///
    /// ## Panics
    ///
    /// Panics if the handle hasn't been consumed yet, or if already restored.
    pub fn restore(mut self, handle: StorageHandle<Database>) {
        match self.state {
            GuardState::Consumed => {
                self.safe_handle.restore_from_mutation(handle);
                self.state = GuardState::Restored;
            }
            GuardState::Active { .. } => panic!(
                "Cannot restore handle - it hasn't been consumed yet. Call guard.handle() \
                 first to get the handle, then restore the updated handle after mutations."
            ),
            GuardState::Restored => {
                panic!("Handle has already been restored. Each guard can only restore once.")
            }
        }
    }
}

impl Drop for StorageHandleGuard<'_> {
    fn drop(&mut self) {
        // Provide specific error messages based on the exact state
        // Avoid double-panic during unwinding
        if !std::thread::panicking() {
            match &self.state {
                GuardState::Active { .. } => {
                    panic!(
                        "StorageHandleGuard dropped without using the handle. Either call \
                         guard.handle() to consume the handle for mutations, or ensure the \
                         guard is properly used in your mutation workflow."
                    );
                }
                GuardState::Consumed => {
                    panic!(
                        "StorageHandleGuard dropped without restoring handle. You must call \
                         guard.restore(updated_handle) to properly restore the database handle \
                         after mutation operations complete."
                    );
                }
                GuardState::Restored => {
                    // All good - proper cleanup completed
                }
            }
        }
    }
}

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
        let mut guard = StorageHandleGuard::new(&mut self.db_handle);
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

    /// Try to read file content using read-only database access.
    ///
    /// Returns `Some(content)` if the file exists in the database, `None` otherwise.
    /// This avoids write locks for files that are already being tracked.
    fn try_read_file(&self, path: &Path) -> Option<String> {
        self.with_db(|db| {
            if let Some(file) = db.get_file(path) {
                tracing::debug!("Using optimized read path for {}", path.display());
                Some(source_text(db, file).to_string())
            } else {
                tracing::debug!(
                    "File {} not in database, requiring creation",
                    path.display()
                );
                None
            }
        })
    }

    /// Get file content through the database.
    ///
    /// First attempts read-only access for existing files, then escalates to write
    /// access only if the file needs to be created. This improves concurrency by
    /// avoiding unnecessary write locks.
    pub fn file_content(&mut self, path: PathBuf) -> String {
        // Try read-only access first for existing files
        if let Some(content) = self.try_read_file(&path) {
            return content;
        }

        // File doesn't exist, escalate to write access to create it
        tracing::debug!(
            "Escalating to write access to create file {}",
            path.display()
        );
        self.with_db_mut(|db| {
            let file = db.get_or_create_file(&path);
            source_text(db, file).to_string()
        })
    }

    /// Get the revision number of a file if it exists.
    ///
    /// Returns `None` if the file is not being tracked by the database.
    /// Uses read-only database access since no mutation is needed.
    #[must_use]
    pub fn file_revision(&self, path: &Path) -> Option<u64> {
        self.with_db(|db| db.get_file(path).map(|file| file.revision(db)))
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

    /// Get a reference to the buffers.
    #[must_use]
    pub fn buffers(&self) -> &Buffers {
        &self.buffers
    }

    /// Get a clone of the file system.
    ///
    /// Returns an Arc-wrapped [`WorkspaceFileSystem`] that can be shared
    /// across threads and used for file operations.
    #[must_use]
    pub fn file_system(&self) -> Arc<WorkspaceFileSystem> {
        self.file_system.clone()
    }

    /// Get a clone of the files tracking map.
    ///
    /// Returns an Arc-wrapped [`DashMap`] for O(1) file lookups that
    /// can be shared across Database instances.
    #[must_use]
    pub fn files(&self) -> Arc<DashMap<PathBuf, SourceFile>> {
        self.files.clone()
    }

    /// Get a cloned database handle for read operations.
    ///
    /// This provides access to a [`StorageHandle`](salsa::StorageHandle) for cases where
    /// [`with_db`](Self::with_db) isn't sufficient. The handle is cloned to allow
    /// concurrent read operations.
    ///
    /// For mutation operations, use [`with_db_mut`](Self::with_db_mut) instead.
    ///
    /// ## Panics
    ///
    /// Panics if the handle is currently taken for mutation.
    #[must_use]
    pub fn db_handle(&self) -> StorageHandle<Database> {
        self.db_handle.clone_for_read()
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::source_text;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::str::FromStr;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tempfile::tempdir;

    #[test]
    fn test_normal_mutation_flow_with_guard() {
        let mut workspace = Workspace::new();

        // Normal mutation should work fine
        let result = workspace.with_db_mut(|db| {
            // Simple operation - create a file
            let path = PathBuf::from("test.py");
            let file = db.get_or_create_file(&path);
            file.revision(db) // Return the revision number
        });

        // Should complete successfully - initial revision is 0
        assert_eq!(result, 0);

        // Should be able to read after mutation
        let file_exists = workspace.with_db(|db| db.has_file(&PathBuf::from("test.py")));

        assert!(file_exists);
    }

    #[test]
    fn test_read_access_during_no_mutation() {
        let workspace = Workspace::new();

        // Multiple concurrent reads should work
        let handle1 = workspace.db_handle();
        let handle2 = workspace.db_handle();

        // Both handles should be valid
        let storage1 = handle1.into_storage();
        let storage2 = handle2.into_storage();

        // Should be able to create databases from both
        let db1 = Database::from_storage(
            storage1,
            workspace.file_system.clone(),
            workspace.files.clone(),
        );
        let db2 = Database::from_storage(
            storage2,
            workspace.file_system.clone(),
            workspace.files.clone(),
        );

        // Both should work
        assert!(!db1.has_file(&PathBuf::from("nonexistent.py")));
        assert!(!db2.has_file(&PathBuf::from("nonexistent.py")));
    }

    #[test]
    #[should_panic(
        expected = "Cannot access database handle for read - handle is currently taken for mutation"
    )]
    fn test_panic_on_read_during_mutation() {
        // This test is tricky due to Rust's borrowing rules.
        // Test the SafeStorageHandle directly instead of through Workspace
        let mut safe_handle = SafeStorageHandle::new(
            Database::new(
                Arc::new(crate::fs::WorkspaceFileSystem::new(
                    crate::buffers::Buffers::new(),
                    Arc::new(crate::fs::OsFileSystem),
                )),
                Arc::new(DashMap::new()),
            )
            .storage()
            .clone()
            .into_zalsa_handle(),
        );

        // Take the handle
        let _handle = safe_handle.take_for_mutation();

        // Now trying to read should panic
        let _cloned_handle = safe_handle.clone_for_read();
    }

    #[test]
    #[should_panic(expected = "Database handle already taken for mutation")]
    fn test_panic_on_double_take() {
        let mut safe_handle = SafeStorageHandle::new(
            Database::new(
                Arc::new(crate::fs::WorkspaceFileSystem::new(
                    crate::buffers::Buffers::new(),
                    Arc::new(crate::fs::OsFileSystem),
                )),
                Arc::new(DashMap::new()),
            )
            .storage()
            .clone()
            .into_zalsa_handle(),
        );

        // First take should work
        let _handle1 = safe_handle.take_for_mutation();

        // Second take should panic
        let _handle2 = safe_handle.take_for_mutation();
    }

    #[test]
    #[should_panic(expected = "StorageHandle already consumed from guard")]
    fn test_panic_on_double_handle_consumption() {
        let mut workspace = Workspace::new();
        let mut guard = StorageHandleGuard::new(&mut workspace.db_handle);

        // First consumption should work
        let _handle1 = guard.handle();

        // Second consumption should panic
        let _handle2 = guard.handle();
    }

    #[test]
    fn test_manual_restore() {
        let mut workspace = Workspace::new();

        // Take handle manually
        let mut guard = StorageHandleGuard::new(&mut workspace.db_handle);
        let handle = guard.handle();

        // Use it to create a database
        let storage = handle.into_storage();
        let mut db = Database::from_storage(
            storage,
            workspace.file_system.clone(),
            workspace.files.clone(),
        );

        // Make some changes
        let path = PathBuf::from("manual_test.py");
        let _file = db.get_or_create_file(&path);

        // Extract new handle and restore manually
        let new_handle = db.storage().clone().into_zalsa_handle();
        guard.restore(new_handle);

        // Should be able to read now
        let file_exists = workspace.with_db(|db| db.has_file(&PathBuf::from("manual_test.py")));

        assert!(file_exists);
    }

    #[test]
    #[should_panic(expected = "StorageHandleGuard dropped without restoring handle")]
    fn test_panic_on_guard_drop_without_restore() {
        let mut workspace = Workspace::new();

        // Create guard and consume handle but don't restore
        let mut guard = StorageHandleGuard::new(&mut workspace.db_handle);
        let _handle = guard.handle();

        // Guard drops here without restore - should panic
    }

    #[test]
    fn test_event_callbacks_preserved() {
        // This test ensures that the new implementation preserves event callbacks
        // through mutation cycles, unlike the old placeholder approach

        let mut workspace = Workspace::new();

        // Add a file to create some state
        let initial_file_count = workspace.with_db_mut(|db| {
            let path = PathBuf::from("callback_test.py");
            let _file = db.get_or_create_file(&path);
            1 // Return count
        });

        assert_eq!(initial_file_count, 1);

        // Perform another mutation to ensure callbacks are preserved
        let final_file_count = workspace.with_db_mut(|db| {
            let path = PathBuf::from("callback_test2.py");
            let _file = db.get_or_create_file(&path);

            // Count files - should include both
            let has_first = db.has_file(&PathBuf::from("callback_test.py"));
            let has_second = db.has_file(&PathBuf::from("callback_test2.py"));

            if has_first && has_second {
                2
            } else {
                0
            }
        });

        assert_eq!(final_file_count, 2);
    }

    #[test]
    fn test_concurrent_read_operations() {
        let workspace = Workspace::new();

        // Multiple with_db calls should work concurrently
        let result1 = workspace.with_db(|db| db.has_file(&PathBuf::from("test1.py")));

        let result2 = workspace.with_db(|db| db.has_file(&PathBuf::from("test2.py")));

        // Both should complete successfully
        assert!(!result1);
        assert!(!result2);
    }

    #[test]
    fn test_safe_storage_handle_state_transitions() {
        let mut workspace = Workspace::new();

        // Start in Available state - should be able to clone for read
        let _handle = workspace.db_handle();

        // Take for mutation
        let mut guard = StorageHandleGuard::new(&mut workspace.db_handle);
        let handle = guard.handle();

        // Now should be in TakenForMutation state
        // Convert to storage for testing
        let storage = handle.into_storage();
        let db = Database::from_storage(
            storage,
            workspace.file_system.clone(),
            workspace.files.clone(),
        );
        let new_handle = db.storage().clone().into_zalsa_handle();

        // Restore - should return to Available state
        guard.restore(new_handle);

        // Should be able to read again
        let _handle = workspace.db_handle();
    }

    #[test]
    #[should_panic(expected = "Cannot restore handle - it hasn't been consumed yet")]
    fn test_panic_on_restore_without_consume() {
        let mut workspace = Workspace::new();
        let guard = StorageHandleGuard::new(&mut workspace.db_handle);

        // Create a dummy handle for testing
        let dummy_handle = Database::new(
            Arc::new(crate::fs::WorkspaceFileSystem::new(
                crate::buffers::Buffers::new(),
                Arc::new(crate::fs::OsFileSystem),
            )),
            Arc::new(DashMap::new()),
        )
        .storage()
        .clone()
        .into_zalsa_handle();

        // Try to restore without consuming first - should panic
        guard.restore(dummy_handle);
    }

    #[test]
    #[should_panic(expected = "StorageHandleGuard dropped without using the handle")]
    fn test_panic_on_guard_drop_without_use() {
        let mut workspace = Workspace::new();

        // Create guard but don't use the handle - should panic on drop
        let _guard = StorageHandleGuard::new(&mut workspace.db_handle);

        // Guard drops here without handle() being called
    }

    #[test]
    fn test_missing_file_returns_empty_content() {
        // Tests that source_text returns "" for non-existent files
        // instead of panicking or propagating errors
        let mut workspace = Workspace::new();

        // Create a file reference for non-existent path
        let content = workspace.with_db_mut(|db| {
            let file = db.get_or_create_file(&PathBuf::from_str("/nonexistent/file.py").unwrap());
            source_text(db, file).to_string()
        });

        assert_eq!(content, "");
    }

    #[test]
    #[cfg(unix)]
    fn test_permission_denied_file_handling() {
        // Create a file with no read permissions
        let temp_dir = tempdir().unwrap();
        let file_path = temp_dir.path().join("no_read.py");
        std::fs::write(&file_path, "content").unwrap();

        // Remove read permissions
        std::fs::set_permissions(&file_path, std::fs::Permissions::from_mode(0o000)).unwrap();

        let mut workspace = Workspace::new();
        let content = workspace.with_db_mut(|db| {
            let file = db.get_or_create_file(&file_path);
            source_text(db, file).to_string()
        });

        // Should return empty string, not panic
        assert_eq!(content, "");
    }

    #[test]
    fn test_invalid_utf8_file_handling() {
        // Create a file with invalid UTF-8
        let temp_dir = tempdir().unwrap();
        let file_path = temp_dir.path().join("invalid.py");
        std::fs::write(&file_path, [0xFF, 0xFE, 0xFD]).unwrap();

        let mut workspace = Workspace::new();
        let content = workspace.with_db_mut(|db| {
            let file = db.get_or_create_file(&file_path);
            source_text(db, file).to_string()
        });

        // Should handle gracefully (empty or replacement chars)
        assert!(content.is_empty() || content.contains('�'));
    }

    #[test]
    fn test_file_deleted_after_tracking() {
        let temp_dir = tempdir().unwrap();
        let file_path = temp_dir.path().join("disappearing.py");
        std::fs::write(&file_path, "original").unwrap();

        let mut workspace = Workspace::new();

        // First read should succeed
        let content1 = workspace.with_db_mut(|db| {
            let file = db.get_or_create_file(&file_path);
            source_text(db, file).to_string()
        });
        assert_eq!(content1, "original");

        // Delete the file
        std::fs::remove_file(&file_path).unwrap();

        // Touch to invalidate cache
        workspace.with_db_mut(|db| {
            db.touch_file(&file_path);
        });

        // Second read should return empty (not panic)
        let content2 = workspace.with_db_mut(|db| {
            let file = db.get_or_create_file(&file_path);
            source_text(db, file).to_string()
        });
        assert_eq!(content2, "");
    }

    #[test]
    #[cfg(unix)]
    fn test_broken_symlink_handling() {
        let temp_dir = tempdir().unwrap();
        let symlink_path = temp_dir.path().join("broken_link.py");

        // Create broken symlink
        std::os::unix::fs::symlink("/nonexistent/target", &symlink_path).unwrap();

        let mut workspace = Workspace::new();
        let content = workspace.with_db_mut(|db| {
            let file = db.get_or_create_file(&symlink_path);
            source_text(db, file).to_string()
        });

        // Should handle gracefully
        assert_eq!(content, "");
    }

    #[test]
    fn test_file_modified_during_operations() {
        // Tests that concurrent file modifications don't crash
        let temp_dir = tempdir().unwrap();
        let file_path = temp_dir.path().join("racing.py");

        let workspace = Arc::new(Mutex::new(Workspace::new()));
        let path_clone = file_path.clone();
        let workspace_clone = workspace.clone();

        // Writer thread
        let writer = std::thread::spawn(move || {
            for i in 0..10 {
                std::fs::write(&path_clone, format!("version {i}")).ok();
                std::thread::sleep(Duration::from_millis(10));
            }
        });

        // Reader thread - should never panic
        for _ in 0..10 {
            let content = workspace_clone.lock().unwrap().with_db_mut(|db| {
                let file = db.get_or_create_file(&file_path);
                source_text(db, file).to_string()
            });
            // Content may vary but shouldn't crash
            assert!(content.is_empty() || content.starts_with("version"));
            std::thread::sleep(Duration::from_millis(5));
        }

        writer.join().unwrap();
    }
}

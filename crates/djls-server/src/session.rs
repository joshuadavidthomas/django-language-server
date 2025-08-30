//! # Salsa StorageHandle Pattern for LSP
//!
//! This module implements a thread-safe Salsa database wrapper for use with
//! tower-lsp's async runtime. The key challenge is that tower-lsp requires
//! `Send + Sync + 'static` bounds, but Salsa's `Storage` contains thread-local
//! state and is not `Send`.
//!
//! ## The Solution: StorageHandle
//!
//! Salsa provides `StorageHandle` which IS `Send + Sync` because it contains
//! no thread-local state. We store the handle and create `Storage`/`Database`
//! instances on-demand.
//!
//! ## The Mutation Challenge
//!
//! When mutating Salsa inputs (e.g., updating file revisions), Salsa must
//! ensure exclusive access to prevent race conditions. It does this via
//! `cancel_others()` which:
//!
//! 1. Sets a cancellation flag (causes other threads to panic with `Cancelled`)
//! 2. Waits for all `StorageHandle` clones to drop
//! 3. Proceeds with the mutation
//!
//! If we accidentally clone the handle instead of taking ownership, step 2
//! never completes → deadlock!
//!
//! ## The Pattern
//!
//! - **Reads**: Clone the handle freely (`with_db`)
//! - **Mutations**: Take exclusive ownership (`with_db_mut` via `take_db_handle_for_mutation`)
//!
//! The explicit method names make the intent clear and prevent accidental misuse.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use dashmap::DashMap;
use djls_conf::Settings;
use djls_project::DjangoProject;
use djls_workspace::{
    db::{Database, SourceFile},
    paths, Buffers, FileSystem, OsFileSystem, TextDocument, WorkspaceFileSystem,
};
use salsa::{Setter, StorageHandle};
use tower_lsp_server::lsp_types;
use url::Url;

/// LSP Session with thread-safe Salsa database access.
///
/// Uses Salsa's `StorageHandle` pattern to maintain `Send + Sync + 'static`
/// compatibility required by tower-lsp. The handle can be safely shared
/// across threads and async boundaries.
///
/// See [this Salsa Zulip discussion](https://salsa.zulipchat.com/#narrow/channel/145099-Using-Salsa/topic/.E2.9C.94.20Advice.20on.20using.20salsa.20from.20Sync.20.2B.20Send.20context/with/495497515)
/// for more information about `StorageHandle`.
///
/// ## Architecture
///
/// Two-layer system inspired by Ruff/Ty:
/// - **Layer 1**: In-memory overlays (LSP document edits)
/// - **Layer 2**: Salsa database (incremental computation cache)
///
/// ## Salsa Mutation Protocol
///
/// When mutating Salsa inputs (like changing file revisions), we must ensure
/// exclusive access to prevent race conditions. Salsa enforces this through
/// its `cancel_others()` mechanism, which waits for all `StorageHandle` clones
/// to drop before allowing mutations.
///
/// We use explicit methods (`take_db_handle_for_mutation`/`restore_db_handle`)
/// to make this ownership transfer clear and prevent accidental deadlocks.
pub struct Session {
    /// The Django project configuration
    project: Option<DjangoProject>,

    /// LSP server settings
    settings: Settings,

    /// Layer 1: Shared buffer storage for open documents
    ///
    /// This implements Ruff's two-layer architecture where Layer 1 contains
    /// open document buffers that take precedence over disk files. The buffers
    /// are shared between Session (which manages them) and WorkspaceFileSystem
    /// (which reads from them).
    ///
    /// Key properties:
    /// - Thread-safe via the Buffers abstraction
    /// - Contains full TextDocument with content, version, and metadata
    /// - Never becomes Salsa inputs - only intercepted at read time
    buffers: Buffers,

    /// File system abstraction with buffer interception
    ///
    /// This WorkspaceFileSystem bridges Layer 1 (buffers) and Layer 2 (Salsa).
    /// It intercepts FileSystem::read_to_string() calls to return buffer
    /// content when available, falling back to disk otherwise.
    file_system: Arc<dyn FileSystem>,

    /// Shared file tracking across all Database instances
    ///
    /// This is the canonical Salsa pattern from the lazy-input example.
    /// The DashMap provides O(1) lookups and is shared via Arc across
    /// all Database instances created from StorageHandle.
    files: Arc<DashMap<PathBuf, SourceFile>>,

    #[allow(dead_code)]
    client_capabilities: lsp_types::ClientCapabilities,

    /// Layer 2: Thread-safe Salsa database handle for pure computation
    ///
    /// where we're using the `StorageHandle` to create a thread-safe handle that can be
    /// shared between threads.
    ///
    /// The database receives file content via the FileSystem trait, which
    /// is intercepted by our LspFileSystem to provide overlay content.
    /// This maintains proper separation between Layer 1 and Layer 2.
    db_handle: StorageHandle<Database>,
}

impl Session {
    pub fn new(params: &lsp_types::InitializeParams) -> Self {
        let project_path = Self::get_project_path(params);

        let (project, settings) = if let Some(path) = &project_path {
            let settings =
                djls_conf::Settings::new(path).unwrap_or_else(|_| djls_conf::Settings::default());

            let project = Some(djls_project::DjangoProject::new(path.clone()));

            (project, settings)
        } else {
            (None, Settings::default())
        };

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
            project,
            settings,
            buffers,
            file_system,
            files,
            client_capabilities: params.capabilities.clone(),
            db_handle,
        }
    }
    /// Determines the project root path from initialization parameters.
    ///
    /// Tries the current directory first, then falls back to the first workspace folder.
    fn get_project_path(params: &lsp_types::InitializeParams) -> Option<PathBuf> {
        // Try current directory first
        std::env::current_dir().ok().or_else(|| {
            // Fall back to the first workspace folder URI
            params
                .workspace_folders
                .as_ref()
                .and_then(|folders| folders.first())
                .and_then(|folder| paths::lsp_uri_to_path(&folder.uri))
        })
    }

    #[must_use]
    pub fn project(&self) -> Option<&DjangoProject> {
        self.project.as_ref()
    }

    pub fn project_mut(&mut self) -> &mut Option<DjangoProject> {
        &mut self.project
    }

    #[must_use]
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn set_settings(&mut self, settings: Settings) {
        self.settings = settings;
    }

    /// Takes exclusive ownership of the database handle for mutation operations.
    ///
    /// This method extracts the `StorageHandle` from the session, replacing it
    /// with a temporary placeholder. This ensures there's exactly one handle
    /// active during mutations, preventing deadlocks in Salsa's `cancel_others()`.
    ///
    /// ## Why Not Clone?
    ///
    /// Cloning would create multiple handles. When Salsa needs to mutate inputs,
    /// it calls `cancel_others()` which waits for all handles to drop. With
    /// multiple handles, this wait would never complete → deadlock.
    ///
    /// ## Panics
    ///
    /// This is an internal method that should only be called by `with_db_mut`.
    /// Multiple concurrent calls would panic when trying to take an already-taken handle.
    fn take_db_handle_for_mutation(&mut self) -> StorageHandle<Database> {
        std::mem::replace(&mut self.db_handle, StorageHandle::new(None))
    }

    /// Restores the database handle after a mutation operation completes.
    ///
    /// This should be called with the handle extracted from the database
    /// after mutations are complete. It updates the session's handle to
    /// reflect any changes made during the mutation.
    fn restore_db_handle(&mut self, handle: StorageHandle<Database>) {
        self.db_handle = handle;
    }

    /// Execute a closure with mutable access to the database.
    ///
    /// This method implements Salsa's required protocol for mutations:
    /// 1. Takes exclusive ownership of the [`StorageHandle`](salsa::StorageHandle)
    ///    (no clones exist)
    /// 2. Creates a temporary Database for the operation
    /// 3. Executes your closure with `&mut Database`
    /// 4. Extracts and restores the updated handle
    ///
    /// ## Example
    ///
    /// ```rust,ignore
    /// session.with_db_mut(|db| {
    ///     let file = db.get_or_create_file(path);
    ///     file.set_revision(db).to(new_revision);  // Mutation requires exclusive access
    /// });
    /// ```
    ///
    /// ## Why This Pattern?
    ///
    /// This ensures that when Salsa needs to modify inputs (via setters like
    /// `set_revision`), it has exclusive access. The internal `cancel_others()`
    /// call will succeed because we guarantee only one handle exists.
    pub fn with_db_mut<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut Database) -> R,
    {
        let handle = self.take_db_handle_for_mutation();

        let storage = handle.into_storage();
        let mut db = Database::from_storage(storage, self.file_system.clone(), self.files.clone());

        let result = f(&mut db);

        // The database may have changed during mutations, so we need
        // to extract its current handle state
        let new_handle = db.storage().clone().into_zalsa_handle();
        self.restore_db_handle(new_handle);

        result
    }

    /// Execute a closure with read-only access to the database.
    ///
    /// For read-only operations, we can safely clone the [`StorageHandle`](salsa::StorageHandle)
    /// since Salsa allows multiple concurrent readers. This is more
    /// efficient than taking exclusive ownership.
    ///
    /// ## Example
    ///
    /// ```rust,ignore
    /// let content = session.with_db(|db| {
    ///     let file = db.get_file(path)?;
    ///     source_text(db, file).to_string()  // Read-only query
    /// });
    /// ```
    pub fn with_db<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Database) -> R,
    {
        // For reads, cloning is safe and efficient
        let storage = self.db_handle.clone().into_storage();
        let db = Database::from_storage(storage, self.file_system.clone(), self.files.clone());
        f(&db)
    }

    /// Handle opening a document - sets buffer and creates file.
    ///
    /// This method coordinates both layers:
    /// - Layer 1: Stores the document content in buffers
    /// - Layer 2: Creates the SourceFile in Salsa (if path is resolvable)
    pub fn open_document(&mut self, url: Url, document: TextDocument) {
        tracing::debug!("Opening document: {}", url);

        // Layer 1: Set buffer
        self.buffers.open(url.clone(), document);

        // Layer 2: Create file and bump revision if it already exists
        // This is crucial: if the file was already read from disk, we need to
        // invalidate Salsa's cache so it re-reads through the buffer system
        if let Some(path) = paths::url_to_path(&url) {
            self.with_db_mut(|db| {
                // Check if file already exists (was previously read from disk)
                let already_exists = db.has_file(&path);
                let file = db.get_or_create_file(path.clone());

                if already_exists {
                    // File was already read - bump revision to invalidate cache
                    let current_rev = file.revision(db);
                    let new_rev = current_rev + 1;
                    file.set_revision(db).to(new_rev);
                    tracing::debug!(
                        "Bumped revision for {} on open: {} -> {}",
                        path.display(),
                        current_rev,
                        new_rev
                    );
                } else {
                    // New file - starts at revision 0
                    tracing::debug!(
                        "Created new SourceFile for {}: revision {}",
                        path.display(),
                        file.revision(db)
                    );
                }
            });
        }
    }

    /// Handle document changes - updates buffer and bumps revision.
    ///
    /// This method coordinates both layers:
    /// - Layer 1: Updates the document content in buffers
    /// - Layer 2: Bumps the file revision to trigger Salsa invalidation
    pub fn update_document(&mut self, url: &Url, document: TextDocument) {
        let version = document.version();
        tracing::debug!("Updating document: {} (version {})", url, version);

        // Layer 1: Update buffer
        self.buffers.update(url.clone(), document);

        // Layer 2: Bump revision to trigger invalidation
        if let Some(path) = paths::url_to_path(url) {
            self.notify_file_changed(&path);
        }
    }

    /// Apply incremental changes to an open document.
    ///
    /// This encapsulates the full update cycle: retrieving the document,
    /// applying changes, updating the buffer, and bumping Salsa revision.
    ///
    /// Returns an error if the document is not currently open.
    pub fn apply_document_changes(
        &mut self,
        url: &Url,
        changes: Vec<lsp_types::TextDocumentContentChangeEvent>,
        new_version: i32,
    ) -> Result<(), String> {
        if let Some(mut document) = self.buffers.get(url) {
            document.update(changes, new_version);
            self.update_document(url, document);
            Ok(())
        } else {
            Err(format!("Document not open: {url}"))
        }
    }

    /// Handle closing a document - removes buffer and bumps revision.
    ///
    /// This method coordinates both layers:
    /// - Layer 1: Removes the buffer (falls back to disk)
    /// - Layer 2: Bumps revision to trigger re-read from disk
    ///
    /// Returns the removed document if it existed.
    pub fn close_document(&mut self, url: &Url) -> Option<TextDocument> {
        tracing::debug!("Closing document: {}", url);

        // Layer 1: Remove buffer
        let removed = self.buffers.close(url);
        if let Some(ref doc) = removed {
            tracing::debug!(
                "Removed buffer for closed document: {} (was version {})",
                url,
                doc.version()
            );
        }

        // Layer 2: Bump revision to trigger re-read from disk
        // We keep the file alive for potential re-opening
        if let Some(path) = paths::url_to_path(url) {
            self.notify_file_changed(&path);
        }

        removed
    }

    /// Internal: Notify that a file's content has changed.
    ///
    /// This bumps the file's revision number in Salsa, which triggers
    /// invalidation of any queries that depend on the file's content.
    fn notify_file_changed(&mut self, path: &Path) {
        self.with_db_mut(|db| {
            // Only bump revision if file is already being tracked
            // We don't create files just for notifications
            if db.has_file(path) {
                let file = db.get_or_create_file(path.to_path_buf());
                let current_rev = file.revision(db);
                let new_rev = current_rev + 1;
                file.set_revision(db).to(new_rev);
                tracing::debug!(
                    "Bumped revision for {}: {} -> {}",
                    path.display(),
                    current_rev,
                    new_rev
                );
            } else {
                tracing::debug!(
                    "File {} not tracked, skipping revision bump",
                    path.display()
                );
            }
        });
    }

    // ===== Safe Query API =====
    // These methods encapsulate all Salsa interactions, preventing the
    // "mixed database instance" bug by never exposing SourceFile or Database.

    /// Get the current content of a file (from overlay or disk).
    ///
    /// This is the safe way to read file content through the system.
    /// The file is created if it doesn't exist, and content is read
    /// through the FileSystem abstraction (overlay first, then disk).
    pub fn file_content(&mut self, path: PathBuf) -> String {
        use djls_workspace::db::source_text;

        self.with_db_mut(|db| {
            let file = db.get_or_create_file(path);
            source_text(db, file).to_string()
        })
    }

    /// Get the current revision of a file, if it's being tracked.
    ///
    /// Returns None if the file hasn't been created yet.
    pub fn file_revision(&mut self, path: &Path) -> Option<u64> {
        self.with_db_mut(|db| {
            db.has_file(path).then(|| {
                let file = db.get_or_create_file(path.to_path_buf());
                file.revision(db)
            })
        })
    }

    /// Check if a file is currently being tracked in Salsa.
    pub fn has_file(&mut self, path: &Path) -> bool {
        self.with_db(|db| db.has_file(path))
    }
}

impl Default for Session {
    fn default() -> Self {
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
            project: None,
            settings: Settings::default(),
            db_handle,
            file_system,
            files,
            buffers,
            client_capabilities: lsp_types::ClientCapabilities::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use djls_workspace::LanguageId;

    #[test]
    fn test_revision_invalidation_chain() {
        use std::path::PathBuf;

        let mut session = Session::default();

        // Create a test file path
        let path = PathBuf::from("/test/template.html");
        let url = Url::parse("file:///test/template.html").unwrap();

        // Open document with initial content
        println!("**[test]** open document with initial content");
        let document = TextDocument::new(
            "<h1>Original Content</h1>".to_string(),
            1,
            LanguageId::Other,
        );
        session.open_document(url.clone(), document);

        // Try to read content - this might be where it hangs
        println!("**[test]** try to read content - this might be where it hangs");
        let content1 = session.file_content(path.clone());
        assert_eq!(content1, "<h1>Original Content</h1>");

        // Update document with new content
        println!("**[test]** Update document with new content");
        let updated_document =
            TextDocument::new("<h1>Updated Content</h1>".to_string(), 2, LanguageId::Other);
        session.update_document(&url, updated_document);

        // Read content again (should get new overlay content due to invalidation)
        println!(
            "**[test]** Read content again (should get new overlay content due to invalidation)"
        );
        let content2 = session.file_content(path.clone());
        assert_eq!(content2, "<h1>Updated Content</h1>");
        assert_ne!(content1, content2);

        // Close document (removes overlay, bumps revision)
        println!("**[test]** Close document (removes overlay, bumps revision)");
        session.close_document(&url);

        // Read content again (should now read from disk, which returns empty for missing files)
        println!(
            "**[test]** Read content again (should now read from disk, which returns empty for missing files)"
        );
        let content3 = session.file_content(path.clone());
        assert_eq!(content3, ""); // No file on disk, returns empty
    }

    #[test]
    fn test_with_db_mut_preserves_files() {
        use std::path::PathBuf;

        let mut session = Session::default();

        // Create multiple files
        let path1 = PathBuf::from("/test/file1.py");
        let path2 = PathBuf::from("/test/file2.py");

        // Create files through safe API
        session.file_content(path1.clone()); // Creates file1
        session.file_content(path2.clone()); // Creates file2

        // Verify files are preserved across operations
        assert!(session.has_file(&path1));
        assert!(session.has_file(&path2));

        // Files should persist even after multiple operations
        let content1 = session.file_content(path1.clone());
        let content2 = session.file_content(path2.clone());

        // Both should return empty (no disk content)
        assert_eq!(content1, "");
        assert_eq!(content2, "");

        // One more verification
        assert!(session.has_file(&path1));
        assert!(session.has_file(&path2));
    }
}

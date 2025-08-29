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
    FileSystem, OsFileSystem, TextDocument, WorkspaceFileSystem,
};
use percent_encoding::percent_decode_str;
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

    /// Layer 1: Thread-safe overlay storage (Arc<DashMap<Url, TextDocument>>)
    ///
    /// This implements Ruff's two-layer architecture where Layer 1 contains
    /// LSP overlays that take precedence over disk files. The overlays map
    /// document URLs to TextDocuments containing current in-memory content.
    ///
    /// Key properties:
    /// - Thread-safe via Arc<DashMap> for Send+Sync requirements
    /// - Contains full TextDocument with content, version, and metadata
    /// - Never becomes Salsa inputs - only intercepted at read time
    overlays: Arc<DashMap<Url, TextDocument>>,

    /// File system abstraction with overlay interception
    ///
    /// This LspFileSystem bridges Layer 1 (overlays) and Layer 2 (Salsa).
    /// It intercepts FileSystem::read_to_string() calls to return overlay
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

        let overlays = Arc::new(DashMap::new());
        let files = Arc::new(DashMap::new());
        let file_system = Arc::new(WorkspaceFileSystem::new(
            overlays.clone(),
            Arc::new(OsFileSystem),
        ));
        let db_handle = Database::new(file_system.clone(), files.clone())
            .storage()
            .clone()
            .into_zalsa_handle();

        Self {
            project,
            settings,
            overlays,
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
                .and_then(|folder| Self::uri_to_pathbuf(&folder.uri))
        })
    }

    /// Converts a `file:` URI into an absolute `PathBuf`.
    fn uri_to_pathbuf(uri: &lsp_types::Uri) -> Option<PathBuf> {
        // Check if the scheme is "file"
        if uri.scheme().is_none_or(|s| s.as_str() != "file") {
            return None;
        }

        // Get the path part as a string
        let encoded_path_str = uri.path().as_str();

        // Decode the percent-encoded path string
        let decoded_path_cow = percent_decode_str(encoded_path_str).decode_utf8_lossy();
        let path_str = decoded_path_cow.as_ref();

        #[cfg(windows)]
        let path_str = {
            // Remove leading '/' for paths like /C:/...
            path_str.strip_prefix('/').unwrap_or(path_str)
        };

        Some(PathBuf::from(path_str))
    }

    pub fn project(&self) -> Option<&DjangoProject> {
        self.project.as_ref()
    }

    pub fn project_mut(&mut self) -> &mut Option<DjangoProject> {
        &mut self.project
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn set_settings(&mut self, settings: Settings) {
        self.settings = settings;
    }

    /// Get a database instance from the session.
    ///
    /// This creates a usable database from the handle, which can be used
    /// to query and update data. The database itself is not Send/Sync,
    /// but the `StorageHandle` is, allowing us to work with tower-lsp-server.
    ///
    /// The database will read files through the LspFileSystem, which
    /// automatically returns overlay content when available.
    ///
    /// CRITICAL: We pass the shared files Arc to preserve file tracking
    /// across Database reconstructions from StorageHandle.
    #[allow(dead_code)]
    pub fn db(&self) -> Database {
        let storage = self.db_handle.clone().into_storage();
        Database::from_storage(storage, self.file_system.clone(), self.files.clone())
    }

    /// Get access to the file system (for Salsa integration)
    #[allow(dead_code)]
    pub fn file_system(&self) -> Arc<dyn FileSystem> {
        self.file_system.clone()
    }

    /// Set or update an overlay for the given document URL
    ///
    /// This implements Layer 1 of Ruff's architecture - storing in-memory
    /// document changes that take precedence over disk content.
    #[allow(dead_code)] // Used in tests
    pub fn set_overlay(&self, url: Url, document: TextDocument) {
        self.overlays.insert(url, document);
    }

    /// Remove an overlay for the given document URL
    ///
    /// After removal, file reads will fall back to disk content.
    #[allow(dead_code)] // Used in tests
    pub fn remove_overlay(&self, url: &Url) -> Option<TextDocument> {
        self.overlays.remove(url).map(|(_, doc)| doc)
    }

    /// Check if an overlay exists for the given URL
    #[allow(dead_code)]
    pub fn has_overlay(&self, url: &Url) -> bool {
        self.overlays.contains_key(url)
    }

    /// Get a copy of an overlay document
    pub fn get_overlay(&self, url: &Url) -> Option<TextDocument> {
        self.overlays.get(url).map(|doc| doc.clone())
    }

    /// Takes exclusive ownership of the database handle for mutation operations.
    ///
    /// This method extracts the `StorageHandle` from the session, replacing it
    /// with a temporary placeholder. This ensures there's exactly one handle
    /// active during mutations, preventing deadlocks in Salsa's `cancel_others()`.
    ///
    /// # Why Not Clone?
    ///
    /// Cloning would create multiple handles. When Salsa needs to mutate inputs,
    /// it calls `cancel_others()` which waits for all handles to drop. With
    /// multiple handles, this wait would never complete → deadlock.
    ///
    /// # Panics
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
    /// 1. Takes exclusive ownership of the StorageHandle (no clones exist)
    /// 2. Creates a temporary Database for the operation
    /// 3. Executes your closure with `&mut Database`
    /// 4. Extracts and restores the updated handle
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// session.with_db_mut(|db| {
    ///     let file = db.get_or_create_file(path);
    ///     file.set_revision(db).to(new_revision);  // Mutation requires exclusive access
    /// });
    /// ```
    ///
    /// # Why This Pattern?
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
    /// For read-only operations, we can safely clone the `StorageHandle`
    /// since Salsa allows multiple concurrent readers. This is more
    /// efficient than taking exclusive ownership.
    ///
    /// # Example
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

    /// Convert a URL to a PathBuf for file operations.
    ///
    /// This is needed to convert between LSP URLs and file paths for
    /// SourceFile creation and tracking.
    pub fn url_to_path(&self, url: &Url) -> Option<PathBuf> {
        // Only handle file:// URLs
        if url.scheme() != "file" {
            return None;
        }

        // Decode and convert to PathBuf
        let path = percent_decode_str(url.path()).decode_utf8().ok()?;

        #[cfg(windows)]
        let path = path.strip_prefix('/').unwrap_or(&path);

        Some(PathBuf::from(path.as_ref()))
    }

    // ===== Document Lifecycle Management =====
    // These methods encapsulate the two-layer architecture coordination:
    // Layer 1 (overlays) and Layer 2 (Salsa revision tracking)

    /// Handle opening a document - sets overlay and creates file.
    ///
    /// This method coordinates both layers:
    /// - Layer 1: Stores the document content in overlays
    /// - Layer 2: Creates the SourceFile in Salsa (if path is resolvable)
    pub fn open_document(&mut self, url: Url, document: TextDocument) {
        tracing::debug!("Opening document: {}", url);

        // Layer 1: Set overlay
        self.overlays.insert(url.clone(), document);

        // Layer 2: Create file if needed (starts at revision 0)
        if let Some(path) = self.url_to_path(&url) {
            self.with_db_mut(|db| {
                let file = db.get_or_create_file(path.clone());
                tracing::debug!(
                    "Created/retrieved SourceFile for {}: revision {}",
                    path.display(),
                    file.revision(db)
                );
            });
        }
    }

    /// Handle document changes - updates overlay and bumps revision.
    ///
    /// This method coordinates both layers:
    /// - Layer 1: Updates the document content in overlays
    /// - Layer 2: Bumps the file revision to trigger Salsa invalidation
    pub fn update_document(&mut self, url: Url, document: TextDocument) {
        let version = document.version();
        tracing::debug!("Updating document: {} (version {})", url, version);

        // Layer 1: Update overlay
        self.overlays.insert(url.clone(), document);

        // Layer 2: Bump revision to trigger invalidation
        if let Some(path) = self.url_to_path(&url) {
            self.notify_file_changed(path);
        }
    }

    /// Handle closing a document - removes overlay and bumps revision.
    ///
    /// This method coordinates both layers:
    /// - Layer 1: Removes the overlay (falls back to disk)
    /// - Layer 2: Bumps revision to trigger re-read from disk
    ///
    /// Returns the removed document if it existed.
    pub fn close_document(&mut self, url: &Url) -> Option<TextDocument> {
        tracing::debug!("Closing document: {}", url);

        // Layer 1: Remove overlay
        let removed = self.overlays.remove(url).map(|(_, doc)| {
            tracing::debug!(
                "Removed overlay for closed document: {} (was version {})",
                url,
                doc.version()
            );
            doc
        });

        // Layer 2: Bump revision to trigger re-read from disk
        // We keep the file alive for potential re-opening
        if let Some(path) = self.url_to_path(url) {
            self.notify_file_changed(path);
        }

        removed
    }

    /// Internal: Notify that a file's content has changed.
    ///
    /// This bumps the file's revision number in Salsa, which triggers
    /// invalidation of any queries that depend on the file's content.
    fn notify_file_changed(&mut self, path: PathBuf) {
        self.with_db_mut(|db| {
            // Only bump revision if file is already being tracked
            // We don't create files just for notifications
            if db.has_file(&path) {
                let file = db.get_or_create_file(path.clone());
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
        let overlays = Arc::new(DashMap::new());
        let files = Arc::new(DashMap::new());
        let file_system = Arc::new(WorkspaceFileSystem::new(
            overlays.clone(),
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
            overlays,
            client_capabilities: lsp_types::ClientCapabilities::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use djls_workspace::LanguageId;

    #[test]
    fn test_session_overlay_management() {
        let session = Session::default();

        let url = Url::parse("file:///test/file.py").unwrap();
        let document = TextDocument::new("print('hello')".to_string(), 1, LanguageId::Python);

        // Initially no overlay
        assert!(!session.has_overlay(&url));
        assert!(session.get_overlay(&url).is_none());

        // Set overlay
        session.set_overlay(url.clone(), document.clone());
        assert!(session.has_overlay(&url));

        let retrieved = session.get_overlay(&url).unwrap();
        assert_eq!(retrieved.content(), document.content());
        assert_eq!(retrieved.version(), document.version());

        // Remove overlay
        let removed = session.remove_overlay(&url).unwrap();
        assert_eq!(removed.content(), document.content());
        assert!(!session.has_overlay(&url));
    }

    #[test]
    fn test_session_two_layer_architecture() {
        let session = Session::default();

        // Verify we have both layers
        let _filesystem = session.file_system(); // Layer 2: FileSystem bridge
        let _db = session.db(); // Layer 2: Salsa database

        // Verify overlay operations work (Layer 1)
        let url = Url::parse("file:///test/integration.py").unwrap();
        let document = TextDocument::new("# Layer 1 content".to_string(), 1, LanguageId::Python);

        session.set_overlay(url.clone(), document);
        assert!(session.has_overlay(&url));

        // FileSystem should now return overlay content through LspFileSystem
        // (This would be tested more thoroughly in integration tests)
    }

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
        session.update_document(url.clone(), updated_document);

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

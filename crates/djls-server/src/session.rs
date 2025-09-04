//! # LSP Session Management
//!
//! This module implements the LSP session abstraction that manages project-specific
//! state and delegates workspace operations to the Workspace facade.

use std::path::Path;
use std::path::PathBuf;

use djls_conf::Settings;
use djls_project::DjangoProject;
use djls_workspace::db::source_text;
use djls_workspace::db::Database;
use djls_workspace::paths;
use djls_workspace::PositionEncoding;
use djls_workspace::TextDocument;
use djls_workspace::Workspace;
use tower_lsp_server::lsp_types;
use url::Url;

/// LSP Session managing project-specific state and workspace operations.
///
/// The Session serves as the main entry point for LSP operations, managing:
/// - Project configuration and settings
/// - Client capabilities and position encoding
/// - Workspace operations (delegated to the Workspace facade)
///
/// All document lifecycle and database operations are delegated to the
/// encapsulated Workspace, which provides thread-safe Salsa database
/// management with proper mutation safety through `StorageHandleGuard`.
pub struct Session {
    /// The Django project configuration
    project: Option<DjangoProject>,

    /// LSP server settings
    settings: Settings,

    /// Workspace facade that encapsulates all workspace-related functionality
    ///
    /// This includes document buffers, file system abstraction, and the Salsa database.
    /// The workspace provides a clean interface for document lifecycle management
    /// and database operations while maintaining proper isolation and thread safety.
    workspace: Workspace,

    #[allow(dead_code)]
    client_capabilities: lsp_types::ClientCapabilities,

    /// Position encoding negotiated with client
    position_encoding: PositionEncoding,
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

        let workspace = Workspace::new();

        // Negotiate position encoding with client
        let position_encoding = PositionEncoding::negotiate(params);

        Self {
            project,
            settings,
            workspace,
            client_capabilities: params.capabilities.clone(),
            position_encoding,
        }
    }
    /// Determines the project root path from initialization parameters.
    ///
    /// Tries workspace folders first (using the first one), then falls back to current directory.
    fn get_project_path(params: &lsp_types::InitializeParams) -> Option<PathBuf> {
        // Try workspace folders first
        params
            .workspace_folders
            .as_ref()
            .and_then(|folders| folders.first())
            .and_then(|folder| paths::lsp_uri_to_path(&folder.uri))
            .or_else(|| {
                // Fall back to current directory
                std::env::current_dir().ok()
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

    #[must_use]
    pub fn position_encoding(&self) -> PositionEncoding {
        self.position_encoding
    }

    /// Execute a closure with mutable access to the database.
    ///
    /// Delegates to the workspace's safe database mutation mechanism.
    pub fn with_db_mut<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut Database) -> R,
    {
        self.workspace.with_db_mut(f)
    }

    /// Execute a closure with read-only access to the database.
    ///
    /// Delegates to the workspace's safe database read mechanism.
    pub fn with_db<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Database) -> R,
    {
        self.workspace.with_db(f)
    }

    /// Handle opening a document - sets buffer and creates file.
    ///
    /// Delegates to the workspace's document management.
    pub fn open_document(&mut self, url: &Url, document: TextDocument) {
        tracing::debug!("Opening document: {}", url);
        self.workspace.open_document(url, document);
    }

    /// Update a document with the given changes.
    ///
    /// Delegates to the workspace's document management.
    pub fn update_document(
        &mut self,
        url: &Url,
        changes: Vec<lsp_types::TextDocumentContentChangeEvent>,
        new_version: i32,
    ) {
        self.workspace.update_document(url, changes, new_version);
    }

    /// Handle closing a document - removes buffer and bumps revision.
    ///
    /// Delegates to the workspace's document management.
    pub fn close_document(&mut self, url: &Url) -> Option<TextDocument> {
        tracing::debug!("Closing document: {}", url);
        self.workspace.close_document(url)
    }

    /// Get an open document from the buffer layer, if it exists.
    ///
    /// Delegates to the workspace's document management.
    #[must_use]
    pub fn get_document(&self, url: &Url) -> Option<TextDocument> {
        self.workspace.get_document(url)
    }

    /// Get the current content of a file (from overlay or disk).
    ///
    /// This is the safe way to read file content through the system.
    /// The file is created if it doesn't exist, and content is read
    /// through the `FileSystem` abstraction (overlay first, then disk).
    pub fn file_content(&mut self, path: PathBuf) -> String {
        self.with_db_mut(|db| {
            let file = db.get_or_create_file(&path);
            source_text(db, file).to_string()
        })
    }

    /// Get the current revision of a file, if it's being tracked.
    ///
    /// Returns None if the file hasn't been created yet.
    #[must_use] pub fn file_revision(&self, path: &Path) -> Option<u64> {
        {
            let this = &self.workspace;
            this.with_db(|db| db.get_file(path).map(|file| file.revision(db)))
        }
    }

    /// Check if a file is currently being tracked in Salsa.
    #[must_use] pub fn has_file(&self, path: &Path) -> bool {
        self.with_db(|db| db.has_file(path))
    }
}

impl Default for Session {
    fn default() -> Self {
        Self {
            project: None,
            settings: Settings::default(),
            workspace: Workspace::new(),
            client_capabilities: Default::default(),
            position_encoding: PositionEncoding::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use djls_workspace::LanguageId;

    #[test]
    fn test_revision_invalidation_chain() {
        let mut session = Session::default();

        let path = PathBuf::from("/test/template.html");
        let url = Url::parse("file:///test/template.html").unwrap();

        // Open document with initial content
        let document = TextDocument::new(
            "<h1>Original Content</h1>".to_string(),
            1,
            LanguageId::Other,
        );
        session.open_document(&url, document);

        let content1 = session.file_content(path.clone());
        assert_eq!(content1, "<h1>Original Content</h1>");

        // Update document with new content using a full replacement change
        let changes = vec![lsp_types::TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "<h1>Updated Content</h1>".to_string(),
        }];
        session.update_document(&url, changes, 2);

        // Read content again (should get new overlay content due to invalidation)
        let content2 = session.file_content(path.clone());
        assert_eq!(content2, "<h1>Updated Content</h1>");
        assert_ne!(content1, content2);

        // Close document (removes overlay, bumps revision)
        session.close_document(&url);

        // Read content again (should now read from disk, which returns empty for missing files)
        let content3 = session.file_content(path.clone());
        assert_eq!(content3, ""); // No file on disk, returns empty
    }

    #[test]
    fn test_with_db_mut_preserves_files() {
        let mut session = Session::default();

        let path1 = PathBuf::from("/test/file1.py");
        let path2 = PathBuf::from("/test/file2.py");

        session.file_content(path1.clone());
        session.file_content(path2.clone());

        // Verify files are preserved across operations
        assert!(session.has_file(&path1));
        assert!(session.has_file(&path2));

        // Files should persist even after multiple operations
        let content1 = session.file_content(path1.clone());
        let content2 = session.file_content(path2.clone());

        // Both should return empty (no disk content)
        assert_eq!(content1, "");
        assert_eq!(content2, "");

        assert!(session.has_file(&path1));
        assert!(session.has_file(&path2));
    }
}

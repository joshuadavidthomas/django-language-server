//! # LSP Session Management
//!
//! This module implements the LSP session abstraction that manages project-specific
//! state and the Salsa database for incremental computation.

use std::path::PathBuf;
use std::sync::Arc;

use dashmap::DashMap;
use djls_conf::Settings;
use djls_project::DjangoProject;
use djls_project::ProjectMetadata;
use djls_workspace::db::SourceFile;
use djls_workspace::paths;
use djls_workspace::PositionEncoding;
use djls_workspace::TextDocument;
use djls_workspace::Workspace;
use pyo3::PyResult;
use tower_lsp_server::lsp_types;
use url::Url;

use crate::db::DjangoDatabase;

/// LSP Session managing project-specific state and database operations.
///
/// The Session serves as the main entry point for LSP operations, managing:
/// - The Salsa database for incremental computation
/// - Project configuration and settings
/// - Client capabilities and position encoding
/// - Workspace operations (buffers and file system)
///
/// Following Ruff's architecture, the concrete database lives at this level
/// and is passed down to operations that need it.
pub struct Session {
    /// The Django project configuration
    project: Option<DjangoProject>,

    /// LSP server settings
    settings: Settings,

    /// Workspace for buffer and file system management
    ///
    /// This manages document buffers and file system abstraction,
    /// but not the database (which is owned directly by Session).
    workspace: Workspace,

    #[allow(dead_code)]
    client_capabilities: lsp_types::ClientCapabilities,

    /// Position encoding negotiated with client
    position_encoding: PositionEncoding,

    db: DjangoDatabase,
}

impl Session {
    pub fn new(params: &lsp_types::InitializeParams) -> Self {
        let project_path = params
            .workspace_folders
            .as_ref()
            .and_then(|folders| folders.first())
            .and_then(|folder| paths::lsp_uri_to_path(&folder.uri))
            .or_else(|| {
                // Fall back to current directory
                std::env::current_dir().ok()
            });

        let (project, settings, metadata) = if let Some(path) = &project_path {
            let settings =
                djls_conf::Settings::new(path).unwrap_or_else(|_| djls_conf::Settings::default());

            let project = Some(djls_project::DjangoProject::new(path.clone()));

            // Create metadata for the project with venv path from settings
            let venv_path = settings.venv_path().map(PathBuf::from);
            let metadata = ProjectMetadata::new(path.clone(), venv_path);

            (project, settings, metadata)
        } else {
            // Default metadata for when there's no project path
            let metadata = ProjectMetadata::new(PathBuf::from("."), None);
            (None, Settings::default(), metadata)
        };

        // Create workspace for buffer management
        let workspace = Workspace::new();

        // Create the concrete database with the workspace's file system and metadata
        let files = Arc::new(DashMap::new());
        let db = DjangoDatabase::new(workspace.file_system(), files, metadata);

        Self {
            db,
            project,
            settings,
            workspace,
            client_capabilities: params.capabilities.clone(),
            position_encoding: PositionEncoding::negotiate(params),
        }
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

    /// Check if the client supports snippet completions
    #[must_use]
    pub fn supports_snippets(&self) -> bool {
        self.client_capabilities
            .text_document
            .as_ref()
            .and_then(|td| td.completion.as_ref())
            .and_then(|c| c.completion_item.as_ref())
            .and_then(|ci| ci.snippet_support)
            .unwrap_or(false)
    }

    /// Execute a read-only operation with access to the database.
    pub fn with_db<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&DjangoDatabase) -> R,
    {
        f(&self.db)
    }

    /// Execute a mutable operation with exclusive access to the database.
    pub fn with_db_mut<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut DjangoDatabase) -> R,
    {
        f(&mut self.db)
    }

    /// Get a reference to the database for project operations.
    pub fn database(&self) -> &DjangoDatabase {
        &self.db
    }

    /// Initialize the project with the database.
    pub fn initialize_project(&mut self) -> PyResult<()> {
        if let Some(project) = self.project.as_mut() {
            project.initialize(&self.db)
        } else {
            Ok(())
        }
    }

    /// Open a document in the session.
    ///
    /// Updates both the workspace buffers and database. Creates the file in
    /// the database or invalidates it if it already exists.
    pub fn open_document(&mut self, url: &Url, document: TextDocument) {
        // Add to workspace buffers
        self.workspace.open_document(url, document);

        // Update database if it's a file URL
        if let Some(path) = paths::url_to_path(url) {
            // Check if file already exists (was previously read from disk)
            let already_exists = self.db.has_file(&path);
            let _file = self.db.get_or_create_file(&path);

            if already_exists {
                // File was already read - touch to invalidate cache
                self.db.touch_file(&path);
            }
        }
    }

    /// Update a document with incremental changes.
    ///
    /// Applies changes to the document and triggers database invalidation.
    pub fn update_document(
        &mut self,
        url: &Url,
        changes: Vec<lsp_types::TextDocumentContentChangeEvent>,
        version: i32,
    ) {
        // Update in workspace
        self.workspace
            .update_document(url, changes, version, self.position_encoding);

        // Touch file in database to trigger invalidation
        if let Some(path) = paths::url_to_path(url) {
            if self.db.has_file(&path) {
                self.db.touch_file(&path);
            }
        }
    }

    pub fn save_document(&mut self, url: &Url) {
        // Touch file in database to trigger re-analysis
        if let Some(path) = paths::url_to_path(url) {
            self.with_db_mut(|db| {
                if db.has_file(&path) {
                    db.touch_file(&path);
                }
            });
        }
    }

    /// Close a document.
    ///
    /// Removes from workspace buffers and triggers database invalidation to fall back to disk.
    pub fn close_document(&mut self, url: &Url) -> Option<TextDocument> {
        let document = self.workspace.close_document(url);

        // Touch file in database to trigger re-read from disk
        if let Some(path) = paths::url_to_path(url) {
            if self.db.has_file(&path) {
                self.db.touch_file(&path);
            }
        }

        document
    }

    /// Get a document from the buffer if it's open.
    #[must_use]
    pub fn get_document(&self, url: &Url) -> Option<TextDocument> {
        self.workspace.get_document(url)
    }

    /// Get or create a file in the database.
    pub fn get_or_create_file(&mut self, path: &PathBuf) -> SourceFile {
        self.db.get_or_create_file(path)
    }

    /// Check if the client supports pull diagnostics.
    ///
    /// Returns true if the client has indicated support for textDocument/diagnostic requests.
    /// When true, the server should not push diagnostics and instead wait for pull requests.
    #[must_use]
    pub fn supports_pull_diagnostics(&self) -> bool {
        self.client_capabilities
            .text_document
            .as_ref()
            .and_then(|td| td.diagnostic.as_ref())
            .is_some()
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new(&lsp_types::InitializeParams::default())
    }
}

#[cfg(test)]
mod tests {
    use djls_workspace::db::source_text;
    use djls_workspace::LanguageId;

    use super::*;

    // Helper function to create a test file path and URL that works on all platforms
    fn test_file_url(filename: &str) -> (PathBuf, Url) {
        // Use an absolute path that's valid on the platform
        #[cfg(windows)]
        let path = PathBuf::from(format!("C:\\temp\\{filename}"));
        #[cfg(not(windows))]
        let path = PathBuf::from(format!("/tmp/{filename}"));

        let url = Url::from_file_path(&path).expect("Failed to create file URL");
        (path, url)
    }

    #[test]
    fn test_session_database_operations() {
        let mut session = Session::default();

        // Can create files in the database
        let path = PathBuf::from("/test.py");
        let file = session.get_or_create_file(&path);

        // Can read file content through database
        let content = session.with_db(|db| source_text(db, file).to_string());
        assert_eq!(content, ""); // Non-existent file returns empty
    }

    #[test]
    fn test_session_document_lifecycle() {
        let mut session = Session::default();
        let (path, url) = test_file_url("test.py");

        // Open document
        let document = TextDocument::new("print('hello')".to_string(), 1, LanguageId::Python);
        session.open_document(&url, document);

        // Should be in workspace buffers
        assert!(session.get_document(&url).is_some());

        // Should be queryable through database
        let file = session.get_or_create_file(&path);
        let content = session.with_db(|db| source_text(db, file).to_string());
        assert_eq!(content, "print('hello')");

        // Close document
        session.close_document(&url);
        assert!(session.get_document(&url).is_none());
    }

    #[test]
    fn test_session_document_update() {
        let mut session = Session::default();
        let (path, url) = test_file_url("test.py");

        // Open with initial content
        let document = TextDocument::new("initial".to_string(), 1, LanguageId::Python);
        session.open_document(&url, document);

        // Update content
        let changes = vec![lsp_types::TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "updated".to_string(),
        }];
        session.update_document(&url, changes, 2);

        // Verify buffer was updated
        let doc = session.get_document(&url).unwrap();
        assert_eq!(doc.content(), "updated");
        assert_eq!(doc.version(), 2);

        // Database should also see updated content
        let file = session.get_or_create_file(&path);
        let content = session.with_db(|db| source_text(db, file).to_string());
        assert_eq!(content, "updated");
    }
}

//! # LSP Session Management
//!
//! This module implements the LSP session abstraction that manages project-specific
//! state and the Salsa database for incremental computation.

use camino::Utf8PathBuf;
use djls_conf::Settings;
use djls_project::Db as ProjectDb;
use djls_source::File;
use djls_source::FileKind;
use djls_source::PositionEncoding;
use djls_workspace::paths;
use djls_workspace::Db as WorkspaceDb;
use djls_workspace::TextDocument;
use djls_workspace::Workspace;
use tower_lsp_server::lsp_types;
use url::Url;

use crate::db::DjangoDatabase;
use crate::encoding::LspPositionEncoding;

/// LSP Session managing project-specific state and database operations.
///
/// The Session serves as the main entry point for LSP operations, managing:
/// - The Salsa database for incremental computation
/// - Client capabilities and position encoding
/// - Workspace operations (buffers and file system)
/// - All Salsa inputs (`SessionState`, Project)
///
/// Following Ruff's architecture, the concrete database lives at this level
/// and is passed down to operations that need it.
pub struct Session {
    /// Workspace for buffer and file system management
    ///
    /// This manages document buffers and file system abstraction,
    /// but not the database (which is owned directly by Session).
    workspace: Workspace,

    client_capabilities: lsp_types::ClientCapabilities,

    /// Position encoding negotiated with client
    position_encoding: PositionEncoding,

    /// The Salsa database for incremental computation
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
                std::env::current_dir()
                    .ok()
                    .and_then(|p| Utf8PathBuf::from_path_buf(p).ok())
            });

        let workspace = Workspace::new();
        let settings = project_path
            .as_ref()
            .and_then(|path| djls_conf::Settings::new(path).ok())
            .unwrap_or_default();

        let db = DjangoDatabase::new(workspace.overlay(), &settings, project_path.as_deref());

        let position_encoding = LspPositionEncoding::from(params)
            .to_position_encoding()
            .unwrap_or_default();

        Self {
            workspace,
            client_capabilities: params.capabilities.clone(),
            position_encoding,
            db,
        }
    }

    #[must_use]
    pub fn db(&self) -> &DjangoDatabase {
        &self.db
    }

    pub fn set_settings(&mut self, settings: Settings) {
        self.db.set_settings(settings);
    }

    #[must_use]
    pub fn position_encoding(&self) -> PositionEncoding {
        self.position_encoding
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

    /// Get the current project for this session
    pub fn project(&self) -> Option<djls_project::Project> {
        self.db.project()
    }

    /// Open a document in the session.
    ///
    /// Updates both the workspace buffers and database. Creates the file in
    /// the database or invalidates it if it already exists.
    /// For template files, immediately triggers parsing and validation.
    pub fn open_document(&mut self, url: &Url, document: TextDocument) {
        if let Some(file) = self.workspace.open_document(&mut self.db, url, document) {
            self.handle_file(file);
        }
    }

    /// Update a document with incremental changes.
    ///
    /// Applies changes to the document and triggers database invalidation.
    /// For template files, immediately triggers parsing and validation.
    pub fn update_document(
        &mut self,
        url: &Url,
        changes: Vec<lsp_types::TextDocumentContentChangeEvent>,
        version: i32,
    ) {
        if let Some(file) = self.workspace.update_document(
            &mut self.db,
            url,
            changes,
            version,
            self.position_encoding,
        ) {
            self.handle_file(file);
        }
    }

    pub fn save_document(&mut self, url: &Url) -> Option<TextDocument> {
        if let Some(file) = self.workspace.save_document(&mut self.db, url) {
            self.handle_file(file);
        }

        self.workspace.get_document(url)
    }

    /// Close a document.
    ///
    /// Removes from workspace buffers and triggers database invalidation to fall back to disk.
    /// For template files, immediately re-parses from disk.
    pub fn close_document(&mut self, url: &Url) -> Option<TextDocument> {
        self.workspace.close_document(&mut self.db, url)
    }

    /// Get a document from the buffer if it's open.
    #[must_use]
    pub fn get_document(&self, url: &Url) -> Option<TextDocument> {
        self.workspace.get_document(url)
    }

    /// Get or create a file in the database.
    pub fn get_or_create_file(&mut self, path: &Utf8PathBuf) -> File {
        self.db.ensure_file_tracked(path.as_path())
    }

    /// Warm template caches and semantic diagnostics for the updated file.
    fn handle_file(&self, file: File) {
        if FileKind::from(file.path(&self.db)) == FileKind::Template {
            if let Some(nodelist) = djls_templates::parse_template(&self.db, file) {
                djls_semantic::validate_nodelist(&self.db, nodelist);
            }
        }
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
}

impl Default for Session {
    fn default() -> Self {
        Self::new(&lsp_types::InitializeParams::default())
    }
}

#[cfg(test)]
mod tests {
    use djls_workspace::LanguageId;

    use super::*;

    // Helper function to create a test file path and URL that works on all platforms
    fn test_file_url(filename: &str) -> (Utf8PathBuf, Url) {
        // Use an absolute path that's valid on the platform
        #[cfg(windows)]
        let path = Utf8PathBuf::from(format!("C:\\temp\\{filename}"));
        #[cfg(not(windows))]
        let path = Utf8PathBuf::from(format!("/tmp/{filename}"));

        let url = Url::from_file_path(&path).expect("Failed to create file URL");
        (path, url)
    }

    #[test]
    fn test_session_database_operations() {
        let mut session = Session::default();

        // Can create files in the database
        let path = Utf8PathBuf::from("/test.py");
        let file = session.get_or_create_file(&path);

        // Can read file content through database
        let content = session.with_db(|db| file.source(db).to_string());
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
        let content = session.with_db(|db| file.source(db).to_string());
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
        let content = session.with_db(|db| file.source(db).to_string());
        assert_eq!(content, "updated");
    }
}

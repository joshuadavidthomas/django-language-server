//! # LSP Session Management
//!
//! This module implements the LSP session abstraction that manages project-specific
//! state and the Salsa database for incremental computation.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use dashmap::DashMap;
use djls_conf::Settings;
use djls_project::Db as ProjectDb;
use djls_project::ProjectMetadata;
use djls_project::TemplateTags;
use djls_workspace::db::SourceFile;
use djls_workspace::paths;
use djls_workspace::PositionEncoding;
use djls_workspace::TextDocument;
use djls_workspace::Workspace;
use salsa::Setter;
use tower_lsp_server::lsp_types;
use url::Url;

use crate::db::DjangoDatabase;

/// Complete LSP session configuration as a Salsa input.
///
/// This contains all external session state including client capabilities,
/// workspace configuration, and server settings.
#[salsa::input]
pub struct SessionState {
    /// The project root path
    #[returns(ref)]
    pub project_root: Option<Arc<str>>,
    /// Client capabilities negotiated during initialization
    pub client_capabilities: lsp_types::ClientCapabilities,
    /// Position encoding negotiated with client
    pub position_encoding: djls_workspace::PositionEncoding,
    /// Server settings from configuration
    pub server_settings: djls_conf::Settings,
    /// Revision number for invalidation tracking
    pub revision: u64,
}

/// LSP Session managing project-specific state and database operations.
///
/// The Session serves as the main entry point for LSP operations, managing:
/// - The Salsa database for incremental computation
/// - Project configuration and settings
/// - Client capabilities and position encoding
/// - Workspace operations (buffers and file system)
/// - All Salsa inputs (`SessionState`, Project)
///
/// Following Ruff's architecture, the concrete database lives at this level
/// and is passed down to operations that need it.
pub struct Session {
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

    /// The Salsa database for incremental computation
    db: DjangoDatabase,

    /// Session state input - complete LSP session configuration
    state: Option<SessionState>,

    /// Cached template tags - Session is the Arc boundary
    template_tags: Option<Arc<TemplateTags>>,
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

        let (settings, metadata) = if let Some(path) = &project_path {
            let settings =
                djls_conf::Settings::new(path).unwrap_or_else(|_| djls_conf::Settings::default());

            // Create metadata for the project with venv path from settings
            let venv_path = settings.venv_path().map(PathBuf::from);
            let metadata = ProjectMetadata::new(path.clone(), venv_path);

            (settings, metadata)
        } else {
            // Default metadata for when there's no project path
            let metadata = ProjectMetadata::new(PathBuf::from("."), None);
            (Settings::default(), metadata)
        };

        // Create workspace for buffer management
        let workspace = Workspace::new();

        // Create the concrete database with the workspace's file system and metadata
        let files = Arc::new(DashMap::new());
        let mut db = DjangoDatabase::new(workspace.file_system(), files, metadata);

        // Create the session state input
        let project_root = project_path
            .as_ref()
            .and_then(|p| p.to_str())
            .map(Arc::from);
        let session_state = SessionState::new(
            &db,
            project_root,
            params.capabilities.clone(),
            PositionEncoding::negotiate(params),
            settings.clone(),
            0,
        );

        // Initialize the project input with correct interpreter spec from settings
        if let Some(root_path) = &project_path {
            let project = db.project(root_path);

            // Update interpreter spec based on VIRTUAL_ENV if available
            if let Ok(virtual_env) = std::env::var("VIRTUAL_ENV") {
                let interpreter = djls_project::Interpreter::VenvPath(virtual_env);
                project.set_interpreter(&mut db).to(interpreter);
            }

            // Update Django settings module override if available
            if let Ok(settings_module) = std::env::var("DJANGO_SETTINGS_MODULE") {
                project
                    .set_settings_module(&mut db)
                    .to(Some(settings_module));
            }

            // Bump revision to invalidate dependent queries
            let current_rev = project.revision(&db);
            project.set_revision(&mut db).to(current_rev + 1);
        }

        Self {
            settings,
            workspace,
            client_capabilities: params.capabilities.clone(),
            position_encoding: PositionEncoding::negotiate(params),
            db,
            state: Some(session_state),
            template_tags: None,
        }
    }

    /// Refresh Django data for the project (template tags, etc.)
    ///
    /// This method caches the template tags in the Session (Arc boundary)
    /// and warms up other tracked functions.
    pub fn refresh_django_data(&mut self) -> Result<()> {
        // Get the unified project input
        let project = self.project();

        // Cache template tags in Session (Arc boundary)
        self.template_tags = djls_project::template_tags(&self.db, project).map(Arc::new);

        // Warm up other tracked functions
        let _ = djls_project::django_available(&self.db, project);
        let _ = djls_project::django_settings_module(&self.db, project);

        Ok(())
    }

    #[must_use]
    pub fn db(&self) -> &DjangoDatabase {
        &self.db
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
    pub fn initialize_project(&mut self) -> Result<()> {
        // Discover Python environment and update inputs
        self.discover_python_environment()?;

        // Refresh Django data using the new inputs
        self.refresh_django_data()?;

        Ok(())
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

    /// Get the session state input
    #[must_use]
    pub fn session_state(&self) -> Option<SessionState> {
        self.state
    }

    /// Update the session state with new settings
    pub fn update_session_state(&mut self, new_settings: Settings) {
        if let Some(session_state) = self.state {
            // Update the settings in the input
            session_state
                .set_server_settings(&mut self.db)
                .to(new_settings.clone());
            // Bump revision to invalidate dependent queries
            let current_rev = session_state.revision(&self.db);
            session_state.set_revision(&mut self.db).to(current_rev + 1);
        }
        self.settings = new_settings;
    }

    /// Get or create unified project input for the current project
    pub fn project(&mut self) -> djls_project::Project {
        let project_root = if let Some(state) = self.state {
            if let Some(root) = state.project_root(&self.db) {
                Path::new(root.as_ref())
            } else {
                self.db.metadata().root().as_path()
            }
        } else {
            self.db.metadata().root().as_path()
        };
        self.db.project(project_root)
    }

    /// Update project configuration when settings change
    pub fn update_project_config(
        &mut self,
        new_venv_path: Option<PathBuf>,
        new_django_settings: Option<String>,
    ) {
        let project = self.project();

        // Update the interpreter spec if venv path is provided
        if let Some(venv_path) = new_venv_path {
            let interpreter_spec =
                djls_project::Interpreter::VenvPath(venv_path.to_string_lossy().to_string());
            project.set_interpreter(&mut self.db).to(interpreter_spec);
        }

        // Update Django settings override if provided
        if let Some(settings) = new_django_settings {
            project.set_settings_module(&mut self.db).to(Some(settings));
        }

        // Bump revision to invalidate dependent queries
        let current_rev = project.revision(&self.db);
        project.set_revision(&mut self.db).to(current_rev + 1);
    }

    /// Discover and update Python environment state
    pub fn discover_python_environment(&mut self) -> Result<()> {
        let project = self.project();

        // Use the new tracked functions to ensure environment discovery
        let _interpreter_path = djls_project::resolve_interpreter(&self.db, project);
        let _env = djls_project::python_environment(&self.db, project);

        Ok(())
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

    /// Get template tags for the current project.
    ///
    /// Returns a reference to the cached template tags, or None if not available.
    /// Session acts as the Arc boundary, so this returns a borrow.
    #[must_use]
    pub fn template_tags(&self) -> Option<&TemplateTags> {
        self.template_tags.as_deref()
    }
}

impl Default for Session {
    fn default() -> Self {
        let mut session = Self::new(&lsp_types::InitializeParams::default());
        session.state = None; // Default session has no state
        session
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

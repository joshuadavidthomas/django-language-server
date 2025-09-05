//! # LSP Session Management
//!
//! This module implements the LSP session abstraction that manages project-specific
//! state and delegates workspace operations to the Workspace facade.

use djls_conf::Settings;
use djls_project::DjangoProject;
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
        let project_path = params
            .workspace_folders
            .as_ref()
            .and_then(|folders| folders.first())
            .and_then(|folder| paths::lsp_uri_to_path(&folder.uri))
            .or_else(|| {
                // Fall back to current directory
                std::env::current_dir().ok()
            });

        let (project, settings) = if let Some(path) = &project_path {
            let settings =
                djls_conf::Settings::new(path).unwrap_or_else(|_| djls_conf::Settings::default());

            let project = Some(djls_project::DjangoProject::new(path.clone()));

            (project, settings)
        } else {
            (None, Settings::default())
        };

        Self {
            project,
            settings,
            workspace: Workspace::new(),
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
        self.workspace.update_document(url, changes, new_version, self.position_encoding);
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
}

impl Default for Session {
    fn default() -> Self {
        Self {
            project: None,
            settings: Settings::default(),
            workspace: Workspace::new(),
            client_capabilities: lsp_types::ClientCapabilities::default(),
            position_encoding: PositionEncoding::default(),
        }
    }
}

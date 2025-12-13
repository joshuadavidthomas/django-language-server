//! # LSP Session Management
//!
//! This module implements the LSP session abstraction that manages project-specific
//! state and the Salsa database for incremental computation.

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::Settings;
use djls_project::Db as ProjectDb;
use djls_source::File;
use djls_source::FileKind;
use djls_workspace::TextDocument;
use djls_workspace::Workspace;
use tower_lsp_server::ls_types;

use crate::client::ClientInfo;
use crate::db::DjangoDatabase;
use crate::ext::InitializeParamsExt;
use crate::ext::TextDocumentContentChangeEventExt;
use crate::ext::TextDocumentItemExt;
use crate::ext::UriExt;

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

    client_info: ClientInfo,

    /// The Salsa database for incremental computation
    db: DjangoDatabase,
}

impl Session {
    #[must_use]
    pub fn new(params: &ls_types::InitializeParams) -> Self {
        let project_path = params
            .workspace_folders
            .as_ref()
            .and_then(|folders| folders.first())
            .and_then(|folder| folder.uri.to_utf8_path_buf())
            .or_else(|| {
                // Fall back to current directory
                std::env::current_dir()
                    .ok()
                    .and_then(|p| Utf8PathBuf::from_path_buf(p).ok())
            });

        let client_options = params.client_options();

        let client_settings = client_options.settings.clone();

        let workspace = Workspace::new();
        let settings = project_path
            .as_ref()
            .and_then(|path| djls_conf::Settings::new(path, Some(client_settings.clone())).ok())
            .unwrap_or(client_settings);

        let db = DjangoDatabase::new(workspace.overlay(), &settings, project_path.as_deref());

        let client_info = ClientInfo::new(
            &params.capabilities,
            params.client_info.as_ref(),
            client_options,
        );

        Self {
            workspace,
            client_info,
            db,
        }
    }

    pub fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot::new(self.db.clone(), self.client_info.clone())
    }

    pub fn client_info(&self) -> &ClientInfo {
        &self.client_info
    }

    pub fn client_capabilities(&self) -> crate::client::ClientCapabilities {
        self.client_info.capabilities()
    }

    pub fn db(&self) -> &DjangoDatabase {
        &self.db
    }

    pub fn db_mut(&mut self) -> &mut DjangoDatabase {
        &mut self.db
    }

    pub fn set_settings(&mut self, settings: Settings) {
        self.db.set_settings(settings);
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
    pub fn open_document(
        &mut self,
        text_document: &ls_types::TextDocumentItem,
    ) -> Option<TextDocument> {
        let Some(path) = text_document.uri.to_utf8_path_buf() else {
            tracing::debug!("Skip opening non-file URI: {}", text_document.uri.as_str());
            return None;
        };

        let kind = text_document.language_id_to_file_kind(self.client_info.client());

        let document = self.workspace.open_document(
            &mut self.db,
            &path,
            &text_document.text,
            text_document.version,
            kind,
        )?;

        self.handle_file(document.file());
        Some(document)
    }

    pub fn save_document(
        &mut self,
        text_document: &ls_types::TextDocumentIdentifier,
    ) -> Option<TextDocument> {
        let Some(path) = text_document.uri.to_utf8_path_buf() else {
            tracing::debug!("Skip saving non-file URI: {}", text_document.uri.as_str());
            return None;
        };

        let document = self.workspace.save_document(&mut self.db, &path)?;
        self.handle_file(document.file());
        Some(document)
    }

    pub fn update_document(
        &mut self,
        text_document: &ls_types::VersionedTextDocumentIdentifier,
        changes: Vec<ls_types::TextDocumentContentChangeEvent>,
    ) -> Option<TextDocument> {
        let Some(path) = text_document.uri.to_utf8_path_buf() else {
            tracing::debug!("Skip updating non-file URI: {}", text_document.uri.as_str());
            return None;
        };

        let document = self.workspace.update_document(
            &mut self.db,
            &path,
            changes.to_document_changes(),
            text_document.version,
            self.client_info.position_encoding(),
        )?;

        self.handle_file(document.file());
        Some(document)
    }

    /// Close a document.
    ///
    /// Removes from workspace buffers and triggers database invalidation to fall back to disk.
    /// For template files, immediately re-parses from disk.
    pub fn close_document(
        &mut self,
        text_document: &ls_types::TextDocumentIdentifier,
    ) -> Option<TextDocument> {
        let Some(path) = text_document.uri.to_utf8_path_buf() else {
            tracing::debug!("Skip closing non-file URI: {}", text_document.uri.as_str());
            return None;
        };

        let document = self.workspace.close_document(&mut self.db, &path)?;

        Some(document)
    }

    /// Get a document from the buffer if it's open.
    pub fn get_document(&self, path: &Utf8Path) -> Option<TextDocument> {
        self.workspace.get_document(path)
    }

    /// Warm template caches and semantic diagnostics for the updated file.
    fn handle_file(&self, file: File) {
        if FileKind::from(file.path(&self.db)) == FileKind::Template {
            if let Some(nodelist) = djls_templates::parse_template(&self.db, file) {
                djls_semantic::validate_nodelist(&self.db, nodelist);
            }
        }
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new(&ls_types::InitializeParams::default())
    }
}

/// Immutable snapshot of session state for background tasks
#[derive(Clone)]
pub struct SessionSnapshot {
    db: DjangoDatabase,
    client_info: ClientInfo,
}

impl SessionSnapshot {
    pub fn new(db: DjangoDatabase, client_info: ClientInfo) -> Self {
        Self { db, client_info }
    }

    pub fn db(&self) -> &DjangoDatabase {
        &self.db
    }

    pub fn client_info(&self) -> &ClientInfo {
        &self.client_info
    }

    pub fn client_capabilities(&self) -> crate::client::ClientCapabilities {
        self.client_info.capabilities()
    }
}

#[cfg(test)]
mod tests {
    use djls_source::Db as SourceDb;

    use super::*;

    // Helper function to create a test file path and URI that works on all platforms
    fn test_file_uri(filename: &str) -> (Utf8PathBuf, ls_types::Uri) {
        // Use an absolute path that's valid on the platform
        #[cfg(windows)]
        let path = Utf8PathBuf::from(format!("C:\\temp\\{filename}"));
        #[cfg(not(windows))]
        let path = Utf8PathBuf::from(format!("/tmp/{filename}"));

        let uri =
            ls_types::Uri::from_file_path(path.as_std_path()).expect("Failed to create file URI");
        (path, uri)
    }

    #[test]
    fn test_session_document_lifecycle() {
        let mut session = Session::default();
        let (path, uri) = test_file_uri("test.py");

        let text_document = ls_types::TextDocumentItem {
            uri: uri.clone(),
            language_id: "python".to_string(),
            version: 1,
            text: "print('hello')".to_string(),
        };
        session.open_document(&text_document);

        assert!(session.get_document(&path).is_some());

        let db = session.db();
        let file = db.get_or_create_file(&path);
        let content = file.source(db).to_string();
        assert_eq!(content, "print('hello')");

        let close_doc = ls_types::TextDocumentIdentifier { uri };
        session.close_document(&close_doc);
        assert!(session.get_document(&path).is_none());
    }

    #[test]
    fn test_session_document_update() {
        let mut session = Session::default();
        let (path, uri) = test_file_uri("test.py");

        let text_document = ls_types::TextDocumentItem {
            uri: uri.clone(),
            language_id: "python".to_string(),
            version: 1,
            text: "initial".to_string(),
        };
        session.open_document(&text_document);

        let changes = vec![ls_types::TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "updated".to_string(),
        }];
        let versioned_document = ls_types::VersionedTextDocumentIdentifier { uri, version: 2 };
        session.update_document(&versioned_document, changes);

        let doc = session.get_document(&path).unwrap();
        assert_eq!(doc.content(), "updated");
        assert_eq!(doc.version(), 2);

        let db = session.db();
        let file = db.get_or_create_file(&path);
        let content = file.source(db).to_string();
        assert_eq!(content, "updated");
    }

    #[test]
    fn test_snapshot_creation() {
        let session = Session::default();
        let snapshot = session.snapshot();

        assert_eq!(
            session.client_info().position_encoding(),
            snapshot.client_info().position_encoding()
        );
        assert_eq!(
            session.project().is_some(),
            snapshot.db().project().is_some()
        );
    }
}

//! # LSP Session Management
//!
//! This module implements the LSP session abstraction that manages project-specific
//! state and the Salsa database for incremental computation.

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_db::DjangoDatabase;
use djls_source::ChangeEvent;
use djls_source::Db as _;
use djls_source::File;
use djls_source::FileStatus;
use djls_source::Offset;
use djls_source::SourceChanges;
use djls_source::Span;
use djls_source::path_to_file;
use tower_lsp_server::ls_types;

use crate::client::ClientInfo;
use crate::document::TextDocument;
use crate::ext::InitializeParamsExt;
use crate::ext::PositionExt;
use crate::ext::TextDocumentContentChangeEventExt;
use crate::ext::TextDocumentItemExt;
use crate::ext::UriExt;
use crate::workspace::Workspace;

/// How many times snapshot-based reads retry after Salsa cancellation before
/// giving up and returning a fallback.
pub(crate) const SNAPSHOT_CANCEL_RETRIES: usize = 2;

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
pub(crate) struct Session {
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
    pub(crate) fn new(params: &ls_types::InitializeParams) -> Self {
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
        let db = DjangoDatabase::new(
            workspace.overlay(),
            &client_settings,
            project_path.as_deref(),
        );

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

    pub(crate) fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot::new(self.db.clone(), self.client_info.clone())
    }

    pub(crate) fn client_info(&self) -> &ClientInfo {
        &self.client_info
    }

    pub(crate) fn db(&self) -> &DjangoDatabase {
        &self.db
    }

    pub(crate) fn db_mut(&mut self) -> &mut DjangoDatabase {
        &mut self.db
    }

    /// Open a document in the session.
    ///
    /// Updates the workspace buffer first, then applies the project-visible
    /// file event against the overlay-backed database.
    pub(crate) fn open_document(
        &mut self,
        text_document: &ls_types::TextDocumentItem,
    ) -> Option<TextDocument> {
        let Some(path) = text_document.uri.to_utf8_path_buf() else {
            tracing::debug!("Skip opening non-file URI: {}", text_document.uri.as_str());
            return None;
        };

        let kind = text_document.language_id_to_file_kind(self.client_info.client());
        let change = self.open_document_change(&path);
        let document =
            self.workspace
                .open_document(&path, &text_document.text, text_document.version, kind);
        SourceChanges::new([change]).apply(&mut self.db);
        Some(document)
    }

    pub(crate) fn save_document(
        &mut self,
        text_document: &ls_types::TextDocumentIdentifier,
    ) -> Option<TextDocument> {
        let Some(path) = text_document.uri.to_utf8_path_buf() else {
            tracing::debug!("Skip saving non-file URI: {}", text_document.uri.as_str());
            return None;
        };

        let document = self.workspace.save_document(&path)?;
        SourceChanges::new([ChangeEvent::ContentChanged(path)]).apply(&mut self.db);
        Some(document)
    }

    pub(crate) fn update_document(
        &mut self,
        text_document: &ls_types::VersionedTextDocumentIdentifier,
        changes: Vec<ls_types::TextDocumentContentChangeEvent>,
    ) -> Option<TextDocument> {
        let Some(path) = text_document.uri.to_utf8_path_buf() else {
            tracing::debug!("Skip updating non-file URI: {}", text_document.uri.as_str());
            return None;
        };

        let change = if self.workspace.get_document(&path).is_some() {
            ChangeEvent::ContentChanged(path.clone())
        } else {
            self.open_document_change(&path)
        };
        let document = self.workspace.update_document(
            &path,
            changes.to_document_changes(),
            text_document.version,
            self.client_info.position_encoding(),
        )?;
        SourceChanges::new([change]).apply(&mut self.db);
        Some(document)
    }

    /// Close a document.
    ///
    /// Removes the document from workspace buffers, invalidates cached source state,
    /// and lets future reads fall back to disk.
    pub(crate) fn close_document(
        &mut self,
        text_document: &ls_types::TextDocumentIdentifier,
    ) -> Option<TextDocument> {
        let Some(path) = text_document.uri.to_utf8_path_buf() else {
            tracing::debug!("Skip closing non-file URI: {}", text_document.uri.as_str());
            return None;
        };

        let change = self.close_document_change(&path);
        let document = self.workspace.close_document(&path)?;
        SourceChanges::new([change]).apply(&mut self.db);

        Some(document)
    }

    fn open_document_change(&self, path: &Utf8Path) -> ChangeEvent {
        if !self.workspace.disk_is_file(path) {
            return ChangeEvent::BecameVisible(path.to_path_buf());
        }

        match self
            .db
            .files()
            .try_file(path)
            .map(|file| file.status(&self.db))
        {
            Some(FileStatus::Exists) => ChangeEvent::Opened(path.to_path_buf()),
            Some(FileStatus::IsADirectory | FileStatus::NotFound) | None => {
                ChangeEvent::BecameVisible(path.to_path_buf())
            }
        }
    }

    fn close_document_change(&self, path: &Utf8Path) -> ChangeEvent {
        if self.workspace.disk_is_file(path) {
            ChangeEvent::ContentChanged(path.to_path_buf())
        } else {
            ChangeEvent::Deleted(path.to_path_buf())
        }
    }

    /// Get a document from the buffer if it's open.
    #[cfg(test)]
    fn get_document(&self, path: &Utf8Path) -> Option<TextDocument> {
        self.workspace.get_document(path)
    }

    /// Get all currently open documents.
    pub(crate) fn open_documents(&self) -> Vec<TextDocument> {
        self.workspace.open_documents()
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new(&ls_types::InitializeParams::default())
    }
}

/// Immutable snapshot of session state.
#[derive(Clone)]
pub(crate) struct SessionSnapshot {
    db: DjangoDatabase,
    client_info: ClientInfo,
}

impl SessionSnapshot {
    fn new(db: DjangoDatabase, client_info: ClientInfo) -> Self {
        Self { db, client_info }
    }

    pub(crate) fn db(&self) -> &DjangoDatabase {
        &self.db
    }

    pub(crate) fn client_info(&self) -> &ClientInfo {
        &self.client_info
    }

    /// Resolve an LSP document request to the tracked file for that URI.
    ///
    /// Open editor buffers are exposed to Salsa through the workspace overlay,
    /// so feature code should read current text through [`File::source`]
    /// instead of reaching back into [`TextDocument`] state.
    pub(crate) fn file_for_document_request(
        &self,
        text_document: &ls_types::TextDocumentIdentifier,
        request: &str,
    ) -> Option<File> {
        let Some(path) = text_document.uri.to_utf8_path_buf() else {
            tracing::debug!(
                "Skipping non-file URI in {} request: {}",
                request,
                text_document.uri.as_str()
            );
            return None;
        };

        path_to_file(&self.db, &path).ok()
    }

    /// Resolve an LSP positioned document request to a tracked file and byte offset.
    pub(crate) fn position_for_document_request(
        &self,
        text_document: &ls_types::TextDocumentIdentifier,
        position: ls_types::Position,
        request: &str,
    ) -> Option<(File, Offset)> {
        let file = self.file_for_document_request(text_document, request)?;
        let source = file.try_source(&self.db).ok()?;
        let line_index = file.line_index(&self.db);
        let offset = position.to_offset(
            source.as_str(),
            line_index,
            self.client_info.position_encoding(),
        );

        Some((file, offset))
    }

    /// Resolve an LSP ranged document request to a tracked file and byte span.
    pub(crate) fn range_for_document_request(
        &self,
        text_document: &ls_types::TextDocumentIdentifier,
        range: ls_types::Range,
        request: &str,
    ) -> Option<(File, Span)> {
        let file = self.file_for_document_request(text_document, request)?;
        let source = file.try_source(&self.db).ok()?;
        let line_index = file.line_index(&self.db);
        let start = range.start.to_offset(
            source.as_str(),
            line_index,
            self.client_info.position_encoding(),
        );
        let end = range.end.to_offset(
            source.as_str(),
            line_index,
            self.client_info.position_encoding(),
        );
        let span = Span::saturating_from_bounds_usize(start.get() as usize, end.get() as usize);

        Some((file, span))
    }
}

#[cfg(test)]
mod tests {
    use djls_project::Db as ProjectDb;
    use djls_project::Interpreter;
    use tempfile::tempdir;

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
        let file = path_to_file(db, &path).expect("open buffer should be visible to the overlay");
        let content = file
            .try_source(db)
            .expect("open buffer should be readable")
            .to_string();
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
        let file = path_to_file(db, &path).expect("open buffer should be visible to the overlay");
        let content = file
            .try_source(db)
            .expect("open buffer should be readable")
            .to_string();
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
            session.db().project().is_some(),
            snapshot.db().project().is_some()
        );
    }

    #[test]
    fn session_new_uses_initial_project_until_django_discovery_loads_settings() {
        let tempdir = tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf()).unwrap();
        let config_extra_path = root.join("config_extra");
        let client_extra_path = root.join("client_extra");
        let venv_path = root.join(".venv");
        std::fs::create_dir_all(config_extra_path.as_std_path()).unwrap();
        std::fs::create_dir_all(client_extra_path.as_std_path()).unwrap();
        std::fs::write(
            root.join(".env").as_std_path(),
            "FROM_ENV=should_not_load\n",
        )
        .unwrap();
        std::fs::write(
            root.join("djls.toml").as_std_path(),
            format!(
                r#"
django_settings_module = "config.settings"
pythonpath = ["{config_extra_path}"]
"#
            ),
        )
        .unwrap();

        let params = ls_types::InitializeParams {
            workspace_folders: Some(vec![ls_types::WorkspaceFolder {
                uri: ls_types::Uri::from_file_path(root.as_std_path()).unwrap(),
                name: "test_project".to_string(),
            }]),
            initialization_options: Some(serde_json::json!({
                "django_settings_module": "client.settings",
                "pythonpath": [client_extra_path.to_string()],
                "venv_path": venv_path.to_string(),
            })),
            ..Default::default()
        };

        let session = Session::new(&params);
        let db = session.db();
        let project = db.project().expect("initialize should create a project");

        assert_eq!(project.root(db), root.as_path());
        assert_eq!(
            project
                .django_settings_module(db)
                .as_ref()
                .map(djls_project::PythonModuleName::as_str),
            Some("client.settings")
        );
        assert_eq!(project.pythonpath(db), &vec![client_extra_path]);
        assert_eq!(project.interpreter(db), &Interpreter::VenvPath(venv_path));
        assert!(project.env_vars(db).is_empty());

        let search_paths: Vec<_> = project
            .search_paths(db)
            .iter()
            .map(|search_path| search_path.path().to_path_buf())
            .collect();
        assert_eq!(search_paths, vec![root]);
    }
}

//! # LSP Session Management
//!
//! This module implements the LSP session abstraction that manages project-specific
//! state and the Salsa database for incremental computation.

#[cfg(test)]
use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::Settings;
use djls_db::DjangoDatabase;
use djls_source::Db as SourceDb;
use djls_source::File;
use djls_source::Offset;
use djls_workspace::TextDocument;
use djls_workspace::Workspace;
use tower_lsp_server::ls_types;

use crate::client::ClientInfo;
use crate::ext::InitializeParamsExt;
use crate::ext::PositionExt;
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
pub(crate) struct Session {
    /// Workspace for buffer and file system management
    ///
    /// This manages document buffers and file system abstraction,
    /// but not the database (which is owned directly by Session).
    workspace: Workspace,

    client_info: ClientInfo,

    /// Workspace roots provided by the client or current directory fallback.
    workspace_roots: Vec<Utf8PathBuf>,

    /// The Salsa database for incremental computation
    db: DjangoDatabase,

    /// Monotonic identity for open-document lifecycle changes.
    document_epoch: u64,
}

impl Session {
    #[must_use]
    pub(crate) fn new(params: &ls_types::InitializeParams) -> Self {
        let workspace_roots = workspace_roots(params);
        let client_options = params.client_options();
        let client_settings = client_options.settings.clone();

        let workspace = Workspace::new();
        let db = DjangoDatabase::new(workspace.overlay(), &client_settings);

        let client_info = ClientInfo::new(
            &params.capabilities,
            params.client_info.as_ref(),
            client_options,
        );

        Self {
            workspace,
            client_info,
            workspace_roots,
            db,
            document_epoch: 0,
        }
    }

    #[cfg(test)]
    fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot::new(self.db.clone(), self.client_info.clone())
    }

    pub(crate) fn client_info(&self) -> &ClientInfo {
        &self.client_info
    }

    pub(crate) fn workspace_roots(&self) -> &[Utf8PathBuf] {
        &self.workspace_roots
    }

    pub(crate) fn configuration_root(&self) -> Utf8PathBuf {
        self.workspace_roots
            .first()
            .cloned()
            .or_else(|| {
                std::env::current_dir()
                    .ok()
                    .and_then(|path| Utf8PathBuf::from_path_buf(path).ok())
            })
            .unwrap_or_else(|| Utf8PathBuf::from("."))
    }

    pub(crate) fn db(&self) -> &DjangoDatabase {
        &self.db
    }

    pub(crate) fn db_mut(&mut self) -> &mut DjangoDatabase {
        &mut self.db
    }

    pub(crate) fn project_db_snapshot_for_observation(&self) -> DjangoDatabase {
        self.db.clone()
    }

    pub(crate) fn set_settings(&mut self, settings: Settings) -> djls_db::SettingsUpdate {
        self.db.set_settings(settings)
    }

    /// Open a document in the session.
    ///
    /// Updates both the workspace buffers and database. Creates the file in
    /// the database or invalidates it if it already exists.
    pub(crate) fn open_document(
        &mut self,
        text_document: &ls_types::TextDocumentItem,
    ) -> Option<TextDocument> {
        let Some(path) = text_document.uri.to_utf8_path_buf() else {
            tracing::debug!("Skip opening non-file URI: {}", text_document.uri.as_str());
            return None;
        };

        let kind = text_document.language_id_to_file_kind(self.client_info.client());

        self.document_epoch += 1;
        self.workspace.open_document(
            &mut self.db,
            &path,
            &text_document.text,
            text_document.version,
            kind,
        )
    }

    pub(crate) fn save_document(
        &mut self,
        text_document: &ls_types::TextDocumentIdentifier,
    ) -> Option<TextDocument> {
        let Some(path) = text_document.uri.to_utf8_path_buf() else {
            tracing::debug!("Skip saving non-file URI: {}", text_document.uri.as_str());
            return None;
        };

        self.workspace.save_document(&mut self.db, &path)
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

        self.document_epoch += 1;
        self.workspace.update_document(
            &mut self.db,
            &path,
            changes.to_document_changes(),
            text_document.version,
            self.client_info.position_encoding(),
        )
    }

    /// Close a document.
    ///
    /// Removes from workspace buffers and triggers database invalidation to fall back to disk.
    /// For template files, immediately re-parses from disk.
    pub(crate) fn close_document(
        &mut self,
        text_document: &ls_types::TextDocumentIdentifier,
    ) -> Option<TextDocument> {
        let Some(path) = text_document.uri.to_utf8_path_buf() else {
            tracing::debug!("Skip closing non-file URI: {}", text_document.uri.as_str());
            return None;
        };

        self.document_epoch += 1;
        let document = self.workspace.close_document(&mut self.db, &path)?;

        Some(document)
    }

    /// Get a document from the buffer if it's open.
    #[cfg(test)]
    fn get_document(&self, path: &Utf8Path) -> Option<TextDocument> {
        self.workspace.get_document(path)
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

        Some(self.db.get_or_create_file(&path))
    }

    /// Resolve an LSP positioned document request to a tracked file and byte offset.
    pub(crate) fn position_for_document_request(
        &self,
        text_document: &ls_types::TextDocumentIdentifier,
        position: ls_types::Position,
        request: &str,
    ) -> Option<(File, Offset)> {
        let file = self.file_for_document_request(text_document, request)?;
        let source = file.source(&self.db);
        let line_index = file.line_index(&self.db);
        let offset = position.to_offset(
            source.as_str(),
            line_index,
            self.client_info.position_encoding(),
        );

        Some((file, offset))
    }

    /// Get all currently open documents.
    pub(crate) fn open_documents(&self) -> Vec<TextDocument> {
        self.workspace
            .buffers()
            .iter()
            .map(|(_path, document)| document)
            .collect()
    }

    pub(crate) fn open_document_freshness(&self, path: &camino::Utf8Path) -> Option<(i32, u64)> {
        self.workspace
            .get_document(path)
            .map(|document| (document.version(), self.document_epoch))
    }
}

fn workspace_roots(params: &ls_types::InitializeParams) -> Vec<Utf8PathBuf> {
    if let Some(roots) = params
        .workspace_folders
        .as_ref()
        .map(|folders| {
            folders
                .iter()
                .filter_map(|folder| folder.uri.to_utf8_path_buf())
                .collect::<Vec<_>>()
        })
        .filter(|roots| !roots.is_empty())
    {
        return roots;
    }

    #[allow(deprecated)]
    if let Some(root) = params
        .root_uri
        .as_ref()
        .and_then(ls_types::Uri::to_utf8_path_buf)
    {
        return vec![root];
    }

    std::env::current_dir()
        .ok()
        .and_then(|path| Utf8PathBuf::from_path_buf(path).ok())
        .into_iter()
        .collect()
}

impl Default for Session {
    fn default() -> Self {
        Self::new(&ls_types::InitializeParams::default())
    }
}

/// Immutable snapshot of session state for tests.
#[cfg(test)]
#[derive(Clone)]
struct SessionSnapshot {
    db: DjangoDatabase,
    client_info: ClientInfo,
}

#[cfg(test)]
impl SessionSnapshot {
    fn new(db: DjangoDatabase, client_info: ClientInfo) -> Self {
        Self { db, client_info }
    }

    fn db(&self) -> &DjangoDatabase {
        &self.db
    }

    fn client_info(&self) -> &ClientInfo {
        &self.client_info
    }
}

#[cfg(test)]
mod tests {
    use djls_semantic::Db as SemanticDb;
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
    fn session_new_initializes_stable_project_facts() {
        let session = Session::default();

        assert!(matches!(
            djls_project::Db::project(session.db()).source_inventory(session.db()),
            djls_project::SourceFileInventory::Unavailable { .. }
        ));
    }

    #[test]
    fn session_new_preserves_all_workspace_folders() {
        let root_a = ls_types::Uri::from_file_path("/tmp/djls-root-a").unwrap();
        let root_b = ls_types::Uri::from_file_path("/tmp/djls-root-b").unwrap();
        let params = ls_types::InitializeParams {
            workspace_folders: Some(vec![
                ls_types::WorkspaceFolder {
                    uri: root_a,
                    name: "root-a".to_string(),
                },
                ls_types::WorkspaceFolder {
                    uri: root_b,
                    name: "root-b".to_string(),
                },
            ]),
            ..Default::default()
        };

        let session = Session::new(&params);

        assert_eq!(
            session.workspace_roots(),
            &[
                Utf8PathBuf::from("/tmp/djls-root-a"),
                Utf8PathBuf::from("/tmp/djls-root-b")
            ]
        );
    }

    #[test]
    fn session_new_uses_client_settings_without_project_config_load() {
        let params = ls_types::InitializeParams {
            initialization_options: Some(serde_json::json!({
                "debug": true,
                "django_settings_module": "client.settings"
            })),
            ..Default::default()
        };

        let session = Session::new(&params);
        let settings = session.db().settings();

        assert!(settings.debug());
        assert_eq!(settings.django_settings_module(), Some("client.settings"));
        assert!(matches!(
            djls_project::Db::project(session.db()).source_inventory(session.db()),
            djls_project::SourceFileInventory::Unavailable { .. }
        ));
    }

    #[test]
    fn configuration_reload_can_update_settings_without_project() {
        let dir = std::env::temp_dir().join(format!("djls-config-reload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("djls.toml"),
            r#"
[diagnostics.severity]
S100 = "warning"
"#,
        )
        .unwrap();
        let root_uri = ls_types::Uri::from_file_path(&dir).unwrap();
        let params = ls_types::InitializeParams {
            workspace_folders: Some(vec![ls_types::WorkspaceFolder {
                uri: root_uri,
                name: "root".to_string(),
            }]),
            ..Default::default()
        };
        let mut session = Session::new(&params);

        let settings = djls_conf::Settings::load(&session.configuration_root(), None).unwrap();
        let update = session.set_settings(settings);

        assert!(update.diagnostics_changed);
        assert_eq!(
            session.db().settings().diagnostics().get_severity("S100"),
            djls_conf::DiagnosticSeverity::Warning
        );
    }

    #[test]
    fn degraded_no_project_template_requests_do_not_panic() {
        let mut session = Session::default();
        let (path, uri) = test_file_uri("degraded_no_project.html");
        let text_document = ls_types::TextDocumentItem {
            uri: uri.clone(),
            language_id: "django-html".to_string(),
            version: 1,
            text: "{% load missing %}\n{% if user %}{{ user|default:'anon' }}{% endif %}"
                .to_string(),
        };
        session.open_document(&text_document);

        let db = session.db();
        assert!(matches!(
            djls_project::Db::project(db).source_inventory(db),
            djls_project::SourceFileInventory::Unavailable {
                issue: djls_project::SourceFilesIssue::NotLoaded,
            }
        ));
        assert!(matches!(
            djls_project::Db::project(db).root_discovery(db),
            djls_project::ProjectRootDiscovery::Absent,
        ));

        let file = db.get_or_create_file(&path);
        let diagnostics = djls_ide::collect_diagnostics(db, file);
        let completions = djls_ide::handle_completion(
            file.source(db).as_str(),
            ls_types::Position::new(0, 3),
            session.client_info().position_encoding(),
            *file.source(db).kind(),
            None,
            Some(db.tag_specs()),
            None,
            false,
        );
        let hover = djls_ide::hover(db, file, djls_source::Offset::new(3));
        let definition = djls_ide::goto_definition(db, file, djls_source::Offset::new(3));
        let references = djls_ide::find_references(db, file, djls_source::Offset::new(3));

        assert!(diagnostics
            .iter()
            .all(|diagnostic| diagnostic.source.is_some()));
        assert!(completions.is_empty() || completions.iter().all(|item| !item.label.is_empty()));
        assert!(hover.is_none());
        assert!(definition.is_none());
        assert!(references.is_none());
    }

    #[test]
    fn degraded_unavailable_discovery_template_requests_do_not_panic() {
        let mut session = Session::default();
        let (path, uri) = test_file_uri("degraded_unavailable_discovery.html");
        let text_document = ls_types::TextDocumentItem {
            uri: uri.clone(),
            language_id: "django-html".to_string(),
            version: 1,
            text: "{% load missing %}\n{% if user %}{{ user|default:'anon' }}{% endif %}"
                .to_string(),
        };
        session.open_document(&text_document);
        djls_project::Db::set_project_root_discovery(
            session.db_mut(),
            djls_project::ProjectRootDiscovery::FixtureDoesNotModelDiscovery,
        );

        let db = session.db();
        assert!(matches!(
            djls_project::Db::project(db).root_discovery(db),
            djls_project::ProjectRootDiscovery::FixtureDoesNotModelDiscovery
        ));

        let file = db.get_or_create_file(&path);
        let diagnostics = djls_ide::collect_diagnostics(db, file);
        let completions = djls_ide::handle_completion(
            file.source(db).as_str(),
            ls_types::Position::new(0, 3),
            session.client_info().position_encoding(),
            *file.source(db).kind(),
            None,
            Some(db.tag_specs()),
            None,
            false,
        );
        let hover = djls_ide::hover(db, file, djls_source::Offset::new(3));
        let definition = djls_ide::goto_definition(db, file, djls_source::Offset::new(3));
        let references = djls_ide::find_references(db, file, djls_source::Offset::new(3));

        assert!(diagnostics
            .iter()
            .all(|diagnostic| diagnostic.source.is_some()));
        assert!(completions.is_empty() || completions.iter().all(|item| !item.label.is_empty()));
        assert!(hover.is_none());
        assert!(definition.is_none());
        assert!(references.is_none());
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
            djls_project::Db::project(session.db()).source_inventory(session.db()),
            djls_project::Db::project(snapshot.db()).source_inventory(snapshot.db())
        );
        assert_eq!(
            djls_project::Db::project(session.db()).root_discovery(session.db()),
            djls_project::Db::project(snapshot.db()).root_discovery(snapshot.db())
        );
    }
}

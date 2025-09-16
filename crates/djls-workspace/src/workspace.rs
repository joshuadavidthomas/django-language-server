//! Workspace facade for managing buffer and file system components
//!
//! This module provides the [`Workspace`] struct that encapsulates buffer
//! management and file system abstraction. The Salsa database is managed
//! at the Session level, following Ruff's architecture pattern.

use std::sync::Arc;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use dashmap::DashMap;
use djls_source::File;
use tower_lsp_server::lsp_types::TextDocumentContentChangeEvent;
use url::Url;

use crate::buffers::Buffers;
use crate::db::Db;
use crate::document::TextDocument;
use crate::fs::FileSystem;
use crate::fs::OsFileSystem;
use crate::fs::WorkspaceFileSystem;
use crate::paths;

/// Result of a workspace operation that affected a tracked file.
#[derive(Clone)]
pub enum WorkspaceFileEvent {
    Created { file: File, path: Utf8PathBuf },
    Updated { file: File, path: Utf8PathBuf },
}

impl WorkspaceFileEvent {
    #[must_use]
    pub fn file(&self) -> File {
        match self {
            Self::Created { file, .. } | Self::Updated { file, .. } => *file,
        }
    }

    #[must_use]
    pub fn path(&self) -> &Utf8Path {
        match self {
            Self::Created { path, .. } | Self::Updated { path, .. } => path.as_path(),
        }
    }
}

/// Workspace facade that manages buffers and file system.
///
/// This struct provides a unified interface for managing document buffers
/// and file system operations. The Salsa database is managed at a higher
/// level (Session) and passed in when needed for operations.
pub struct Workspace {
    /// Thread-safe shared buffer storage for open documents
    buffers: Buffers,
    /// Registry mapping file paths to Salsa [`File`] handles
    files: Arc<DashMap<Utf8PathBuf, File>>,
    /// File system abstraction that checks buffers first, then disk
    file_system: Arc<WorkspaceFileSystem>,
}

impl Workspace {
    /// Create a new [`Workspace`] with buffers and file system initialized.
    #[must_use]
    pub fn new() -> Self {
        let buffers = Buffers::new();
        let files = Arc::new(DashMap::new());
        let file_system = Arc::new(WorkspaceFileSystem::new(
            buffers.clone(),
            Arc::new(OsFileSystem),
        ));

        Self {
            buffers,
            files,
            file_system,
        }
    }

    /// Get the file system for this workspace.
    ///
    /// The file system checks buffers first, then falls back to disk.
    #[must_use]
    pub fn file_system(&self) -> Arc<dyn FileSystem> {
        self.file_system.clone()
    }

    /// Get the buffers for direct access.
    #[must_use]
    pub fn buffers(&self) -> &Buffers {
        &self.buffers
    }

    /// Get the shared file registry (primarily for tests and database construction).
    #[must_use]
    pub fn files(&self) -> Arc<DashMap<Utf8PathBuf, File>> {
        self.files.clone()
    }

    /// Open a document in the workspace and ensure a corresponding Salsa file exists.
    pub fn open_document(
        &mut self,
        db: &mut dyn Db,
        url: &Url,
        document: TextDocument,
    ) -> Option<WorkspaceFileEvent> {
        self.buffers.open(url.clone(), document);
        self.ensure_file_for_url(db, url).inspect(|event| {
            if let WorkspaceFileEvent::Updated { file, .. } = event {
                db.touch_file(*file);
            }
        })
    }

    /// Update a document with incremental changes and touch the associated file.
    pub fn update_document(
        &mut self,
        db: &mut dyn Db,
        url: &Url,
        changes: Vec<TextDocumentContentChangeEvent>,
        version: i32,
        encoding: crate::encoding::PositionEncoding,
    ) -> Option<WorkspaceFileEvent> {
        if let Some(mut document) = self.buffers.get(url) {
            document.update(changes, version, encoding);
            self.buffers.update(url.clone(), document);
        } else if let Some(first_change) = changes.into_iter().next() {
            if first_change.range.is_none() {
                let document = TextDocument::new(
                    first_change.text,
                    version,
                    crate::language::LanguageId::Other,
                );
                self.buffers.open(url.clone(), document);
            }
        }

        self.ensure_file_for_url(db, url).inspect(|event| {
            db.touch_file(event.file());
        })
    }

    /// Touch the tracked file when the client saves the document.
    pub fn save_document(&self, db: &mut dyn Db, url: &Url) -> Option<WorkspaceFileEvent> {
        let path = paths::url_to_path(url)?;
        let (file, existed) = self.ensure_file(db, &path);
        db.touch_file(file);
        Some(if existed {
            WorkspaceFileEvent::Updated { file, path }
        } else {
            WorkspaceFileEvent::Created { file, path }
        })
    }

    /// Close a document, removing it from buffers and touching the tracked file.
    pub fn close_document(&mut self, db: &mut dyn Db, url: &Url) -> Option<TextDocument> {
        if let Some(path) = paths::url_to_path(url) {
            if let Some(file) = self.get_file(&path) {
                db.touch_file(file);
            }
        }

        self.buffers.close(url)
    }

    /// Get a document from the buffer if it's open.
    #[must_use]
    pub fn get_document(&self, url: &Url) -> Option<TextDocument> {
        self.buffers.get(url)
    }

    /// Get a tracked file for the given path if one exists.
    #[must_use]
    pub fn get_file(&self, path: &Utf8Path) -> Option<File> {
        self.files.get(path).map(|entry| *entry)
    }

    /// Ensure a file exists for the given path without manipulating buffers.
    pub fn ensure_file_by_path(&self, db: &mut dyn Db, path: &Utf8PathBuf) -> WorkspaceFileEvent {
        let (file, existed) = self.ensure_file(db, path);
        if existed {
            WorkspaceFileEvent::Updated {
                file,
                path: path.clone(),
            }
        } else {
            WorkspaceFileEvent::Created {
                file,
                path: path.clone(),
            }
        }
    }

    fn ensure_file_for_url(&self, db: &mut dyn Db, url: &Url) -> Option<WorkspaceFileEvent> {
        let path = paths::url_to_path(url)?;
        let (file, existed) = self.ensure_file(db, &path);
        Some(if existed {
            WorkspaceFileEvent::Updated { file, path }
        } else {
            WorkspaceFileEvent::Created { file, path }
        })
    }

    fn ensure_file(&self, db: &mut dyn Db, path: &Utf8PathBuf) -> (File, bool) {
        if let Some(entry) = self.files.get(path) {
            return (*entry, true);
        }

        let file = File::new(db, path.clone(), 0);
        self.files.insert(path.clone(), file);
        (file, false)
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use camino::Utf8Path;
    use tempfile::tempdir;
    use url::Url;

    use super::*;
    use crate::encoding::PositionEncoding;
    use crate::InMemoryFileSystem;
    use crate::LanguageId;

    #[salsa::db]
    #[derive(Clone)]
    struct TestDb {
        storage: salsa::Storage<Self>,
        fs: Arc<dyn FileSystem>,
    }

    impl TestDb {
        fn new() -> Self {
            Self {
                storage: salsa::Storage::default(),
                fs: Arc::new(InMemoryFileSystem::new()),
            }
        }
    }

    #[salsa::db]
    impl salsa::Database for TestDb {}

    #[salsa::db]
    impl djls_source::Db for TestDb {
        fn read_file_source(&self, _path: &Utf8Path) -> std::io::Result<String> {
            Ok(String::new())
        }
    }

    #[salsa::db]
    impl crate::db::Db for TestDb {
        fn fs(&self) -> Arc<dyn FileSystem> {
            self.fs.clone()
        }
    }

    #[test]
    fn test_open_document() {
        let mut workspace = Workspace::new();
        let mut db = TestDb::new();
        let url = Url::parse("file:///test.py").unwrap();

        let document = TextDocument::new("print('hello')".to_string(), 1, LanguageId::Python);
        let event = workspace.open_document(&mut db, &url, document).unwrap();

        match event {
            WorkspaceFileEvent::Created { ref path, .. } => {
                assert_eq!(path.file_name(), Some("test.py"));
            }
            WorkspaceFileEvent::Updated { .. } => panic!("expected created event"),
        }
        assert!(workspace.buffers.get(&url).is_some());
    }

    #[test]
    fn test_update_document() {
        let mut workspace = Workspace::new();
        let mut db = TestDb::new();
        let url = Url::parse("file:///test.py").unwrap();

        let document = TextDocument::new("initial".to_string(), 1, LanguageId::Python);
        workspace.open_document(&mut db, &url, document);

        let changes = vec![TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "updated".to_string(),
        }];
        let event = workspace
            .update_document(&mut db, &url, changes, 2, PositionEncoding::Utf16)
            .unwrap();

        assert!(matches!(event, WorkspaceFileEvent::Updated { .. }));
        let buffer = workspace.buffers.get(&url).unwrap();
        assert_eq!(buffer.content(), "updated");
        assert_eq!(buffer.version(), 2);
    }

    #[test]
    fn test_close_document() {
        let mut workspace = Workspace::new();
        let mut db = TestDb::new();
        let url = Url::parse("file:///test.py").unwrap();

        let document = TextDocument::new("content".to_string(), 1, LanguageId::Python);
        workspace.open_document(&mut db, &url, document.clone());

        let closed = workspace.close_document(&mut db, &url);
        assert!(closed.is_some());
        assert!(workspace.buffers.get(&url).is_none());
    }

    #[test]
    fn test_file_system_checks_buffers_first() {
        let temp_dir = tempdir().unwrap();
        let file_path = temp_dir.path().join("test.py");
        std::fs::write(&file_path, "disk content").unwrap();

        let mut workspace = Workspace::new();
        let mut db = TestDb::new();
        let url = Url::from_file_path(&file_path).unwrap();

        let document = TextDocument::new("buffer content".to_string(), 1, LanguageId::Python);
        workspace.open_document(&mut db, &url, document);

        let content = workspace
            .file_system()
            .read_to_string(Utf8Path::from_path(&file_path).unwrap())
            .unwrap();
        assert_eq!(content, "buffer content");
    }
}

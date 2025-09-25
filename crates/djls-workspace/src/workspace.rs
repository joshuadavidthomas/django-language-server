//! Workspace facade for managing buffer and file system components
//!
//! This module provides the [`Workspace`] struct that encapsulates buffer
//! management and file system abstraction. The Salsa database is managed
//! at the Session level, following Ruff's architecture pattern.

use std::io;
use std::sync::Arc;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::File;
use djls_source::FileSystem;
use djls_source::FxDashMap;
use djls_source::OsFileSystem;
use djls_source::PositionEncoding;
use tower_lsp_server::lsp_types::TextDocumentContentChangeEvent;
use url::Url;

use crate::buffers::Buffers;
use crate::db::Db;
use crate::document::TextDocument;
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
///
/// LSP file system that intercepts reads for buffered files.
///
/// This implements a two-layer architecture where Layer 1 (open [`Buffers`])
/// takes precedence over Layer 2 (Salsa database). When a file is read,
/// this system first checks for a buffer (in-memory content from
/// [`TextDocument`](crate::document::TextDocument)) and returns that content.
/// If no buffer exists, it falls back to reading from disk.
///
/// ## Overlay Semantics
///
/// Files in the overlay (buffered files) are treated as first-class files:
/// - `exists()` returns true for overlay files even if they don't exist on disk
/// - `read_to_string()` returns the overlay content
///
/// This ensures consistent behavior across all filesystem operations for
/// buffered files that may not yet be saved to disk.
///
/// This type is used by the database implementations to ensure all file reads go
/// through the buffer system first.
pub struct WorkspaceFileSystem {
    /// In-memory buffers that take precedence over disk files
    buffers: Buffers,
    /// Fallback file system for disk operations
    disk: Arc<dyn FileSystem>,
}

impl WorkspaceFileSystem {
    #[must_use]
    pub fn new(buffers: Buffers, disk: Arc<dyn FileSystem>) -> Self {
        Self { buffers, disk }
    }
}

impl FileSystem for WorkspaceFileSystem {
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
        if let Some(url) = paths::path_to_url(path) {
            if let Some(document) = self.buffers.get(&url) {
                return Ok(document.content().to_string());
            }
        }
        self.disk.read_to_string(path)
    }

    fn exists(&self, path: &Utf8Path) -> bool {
        paths::path_to_url(path).is_some_and(|url| self.buffers.contains(&url))
            || self.disk.exists(path)
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
    files: Arc<FxDashMap<Utf8PathBuf, File>>,
    /// File system abstraction that checks buffers first, then disk
    file_system: Arc<WorkspaceFileSystem>,
}

impl Workspace {
    /// Create a new [`Workspace`] with buffers and file system initialized.
    #[must_use]
    pub fn new() -> Self {
        let buffers = Buffers::new();
        let files = Arc::new(FxDashMap::default());
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

    /// Open a document in the workspace and ensure a corresponding Salsa file exists.
    pub fn open_document(
        &mut self,
        db: &mut dyn Db,
        url: &Url,
        document: TextDocument,
    ) -> Option<WorkspaceFileEvent> {
        self.buffers.open(url.clone(), document);
        self.ensure_file_for_url(db, url).inspect(|event| {
            db.touch_file(event.file());
        })
    }

    /// Update a document with incremental changes and touch the associated file.
    pub fn update_document(
        &mut self,
        db: &mut dyn Db,
        url: &Url,
        changes: Vec<TextDocumentContentChangeEvent>,
        version: i32,
        encoding: PositionEncoding,
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

    /// Ensure a file is tracked in Salsa and report its state.
    pub fn track_file(&self, db: &mut dyn Db, path: &Utf8Path) -> WorkspaceFileEvent {
        let path_buf = path.to_owned();
        let (file, existed) = self.ensure_file(db, &path_buf);
        if existed {
            WorkspaceFileEvent::Updated {
                file,
                path: path_buf,
            }
        } else {
            WorkspaceFileEvent::Created {
                file,
                path: path_buf,
            }
        }
    }

    /// Touch the tracked file when the client saves the document.
    pub fn save_document(&self, db: &mut dyn Db, url: &Url) -> Option<WorkspaceFileEvent> {
        let path = paths::url_to_path(url)?;

        let event = self.track_file(db, path.as_path());
        db.touch_file(event.file());
        Some(event)
    }

    /// Close a document, removing it from buffers and touching the tracked file.
    pub fn close_document(&mut self, db: &mut dyn Db, url: &Url) -> Option<TextDocument> {
        let closed = self.buffers.close(url);

        if let Some(path) = paths::url_to_path(url) {
            if let Some(file) = self.files.get(&path) {
                db.touch_file(*file);
            }
        }

        closed
    }

    /// Get a document from the buffer if it's open.
    #[must_use]
    pub fn get_document(&self, url: &Url) -> Option<TextDocument> {
        self.buffers.get(url)
    }

    fn ensure_file_for_url(&self, db: &mut dyn Db, url: &Url) -> Option<WorkspaceFileEvent> {
        let path = paths::url_to_path(url)?;
        Some(self.track_file(db, path.as_path()))
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
    use super::*;

    mod file_system {
        use camino::Utf8PathBuf;
        use djls_source::InMemoryFileSystem;
        use url::Url;

        use super::*;
        use crate::buffers::Buffers;
        use crate::document::TextDocument;
        use crate::language::LanguageId;

        // Helper to create platform-appropriate test paths
        fn test_file_path(name: &str) -> Utf8PathBuf {
            #[cfg(windows)]
            return Utf8PathBuf::from(format!("C:\\temp\\{name}"));
            #[cfg(not(windows))]
            return Utf8PathBuf::from(format!("/tmp/{name}"));
        }

        #[test]
        fn test_reads_from_buffer_when_present() {
            let disk = Arc::new(InMemoryFileSystem::new());
            let buffers = Buffers::new();
            let fs = WorkspaceFileSystem::new(buffers.clone(), disk);

            // Add file to buffer
            let path = test_file_path("test.py");
            let url = Url::from_file_path(&path).unwrap();
            let doc = TextDocument::new("buffer content".to_string(), 1, LanguageId::Python);
            buffers.open(url, doc);

            assert_eq!(fs.read_to_string(&path).unwrap(), "buffer content");
        }

        #[test]
        fn test_reads_from_disk_when_no_buffer() {
            let mut disk_fs = InMemoryFileSystem::new();
            let path = test_file_path("test.py");
            disk_fs.add_file(path.clone(), "disk content".to_string());

            let buffers = Buffers::new();
            let fs = WorkspaceFileSystem::new(buffers, Arc::new(disk_fs));

            assert_eq!(fs.read_to_string(&path).unwrap(), "disk content");
        }

        #[test]
        fn test_buffer_overrides_disk() {
            let mut disk_fs = InMemoryFileSystem::new();
            let path = test_file_path("test.py");
            disk_fs.add_file(path.clone(), "disk content".to_string());

            let buffers = Buffers::new();
            let fs = WorkspaceFileSystem::new(buffers.clone(), Arc::new(disk_fs));

            // Add buffer with different content
            let url = Url::from_file_path(&path).unwrap();
            let doc = TextDocument::new("buffer content".to_string(), 1, LanguageId::Python);
            buffers.open(url, doc);

            assert_eq!(fs.read_to_string(&path).unwrap(), "buffer content");
        }

        #[test]
        fn test_exists_for_buffer_only_file() {
            let disk = Arc::new(InMemoryFileSystem::new());
            let buffers = Buffers::new();
            let fs = WorkspaceFileSystem::new(buffers.clone(), disk);

            // Add file to buffer only
            let path = test_file_path("buffer_only.py");
            let url = Url::from_file_path(&path).unwrap();
            let doc = TextDocument::new("content".to_string(), 1, LanguageId::Python);
            buffers.open(url, doc);

            assert!(fs.exists(&path));
        }

        #[test]
        fn test_exists_for_disk_only_file() {
            let mut disk_fs = InMemoryFileSystem::new();
            let path = test_file_path("disk_only.py");
            disk_fs.add_file(path.clone(), "content".to_string());

            let buffers = Buffers::new();
            let fs = WorkspaceFileSystem::new(buffers, Arc::new(disk_fs));

            assert!(fs.exists(&path));
        }

        #[test]
        fn test_exists_for_both_buffer_and_disk() {
            let mut disk_fs = InMemoryFileSystem::new();
            let path = test_file_path("both.py");
            disk_fs.add_file(path.clone(), "disk".to_string());

            let buffers = Buffers::new();
            let fs = WorkspaceFileSystem::new(buffers.clone(), Arc::new(disk_fs));

            // Also add to buffer
            let url = Url::from_file_path(&path).unwrap();
            let doc = TextDocument::new("buffer".to_string(), 1, LanguageId::Python);
            buffers.open(url, doc);

            assert!(fs.exists(&path));
        }

        #[test]
        fn test_exists_returns_false_when_nowhere() {
            let disk = Arc::new(InMemoryFileSystem::new());
            let buffers = Buffers::new();
            let fs = WorkspaceFileSystem::new(buffers, disk);

            let path = test_file_path("nowhere.py");
            assert!(!fs.exists(&path));
        }

        #[test]
        fn test_read_error_when_file_nowhere() {
            let disk = Arc::new(InMemoryFileSystem::new());
            let buffers = Buffers::new();
            let fs = WorkspaceFileSystem::new(buffers, disk);

            let path = test_file_path("missing.py");
            let result = fs.read_to_string(&path);
            assert!(result.is_err());
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotFound);
        }

        #[test]
        fn test_reflects_buffer_updates() {
            let disk = Arc::new(InMemoryFileSystem::new());
            let buffers = Buffers::new();
            let fs = WorkspaceFileSystem::new(buffers.clone(), disk);

            let path = test_file_path("test.py");
            let url = Url::from_file_path(&path).unwrap();

            // Initial buffer content
            let doc1 = TextDocument::new("version 1".to_string(), 1, LanguageId::Python);
            buffers.open(url.clone(), doc1);
            assert_eq!(fs.read_to_string(&path).unwrap(), "version 1");

            // Update buffer content
            let doc2 = TextDocument::new("version 2".to_string(), 2, LanguageId::Python);
            buffers.update(url, doc2);
            assert_eq!(fs.read_to_string(&path).unwrap(), "version 2");
        }

        #[test]
        fn test_handles_buffer_removal() {
            let mut disk_fs = InMemoryFileSystem::new();
            let path = test_file_path("test.py");
            disk_fs.add_file(path.clone(), "disk content".to_string());

            let buffers = Buffers::new();
            let fs = WorkspaceFileSystem::new(buffers.clone(), Arc::new(disk_fs));

            let url = Url::from_file_path(&path).unwrap();

            // Add buffer
            let doc = TextDocument::new("buffer content".to_string(), 1, LanguageId::Python);
            buffers.open(url.clone(), doc);
            assert_eq!(fs.read_to_string(&path).unwrap(), "buffer content");

            // Remove buffer
            let _ = buffers.close(&url);
            assert_eq!(fs.read_to_string(&path).unwrap(), "disk content");
        }
    }

    mod workspace {
        use std::sync::Arc;

        use camino::Utf8Path;
        use camino::Utf8PathBuf;
        use tempfile::tempdir;
        use url::Url;

        use super::*;
        use crate::LanguageId;

        #[salsa::db]
        #[derive(Clone)]
        struct TestDb {
            storage: salsa::Storage<Self>,
            fs: Arc<dyn FileSystem>,
        }

        impl TestDb {
            fn new(fs: Arc<dyn FileSystem>) -> Self {
                Self {
                    storage: salsa::Storage::default(),
                    fs,
                }
            }
        }

        #[salsa::db]
        impl salsa::Database for TestDb {}

        #[salsa::db]
        impl djls_source::Db for TestDb {
            fn read_file_source(&self, path: &Utf8Path) -> std::io::Result<String> {
                self.fs.read_to_string(path)
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
            let mut db = TestDb::new(workspace.file_system());
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
            let mut db = TestDb::new(workspace.file_system());
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
            let mut db = TestDb::new(workspace.file_system());
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
            let mut db = TestDb::new(workspace.file_system());
            let url = Url::from_file_path(&file_path).unwrap();

            let document = TextDocument::new("buffer content".to_string(), 1, LanguageId::Python);
            workspace.open_document(&mut db, &url, document);

            let content = workspace
                .file_system()
                .read_to_string(Utf8Path::from_path(&file_path).unwrap())
                .unwrap();
            assert_eq!(content, "buffer content");
        }

        #[test]
        fn test_file_source_reads_from_buffer() {
            let mut workspace = Workspace::new();
            let mut db = TestDb::new(workspace.file_system());

            let temp_dir = tempdir().unwrap();
            let file_path =
                Utf8PathBuf::from_path_buf(temp_dir.path().join("template.html")).unwrap();
            std::fs::write(file_path.as_std_path(), "disk template").unwrap();
            let url = Url::from_file_path(file_path.as_std_path()).unwrap();

            let document = TextDocument::new("line1\nline2".to_string(), 1, LanguageId::HtmlDjango);
            let event = workspace
                .open_document(&mut db, &url, document.clone())
                .unwrap();
            let file = event.file();

            let source = file.source(&db);
            assert_eq!(source.as_str(), document.content());

            let line_index = file.line_index(&db);
            assert_eq!(
                line_index.to_line_col(djls_source::Offset::new(0)),
                djls_source::LineCol::new(0, 0)
            );
            assert_eq!(
                line_index.to_line_col(djls_source::Offset::new(6)),
                djls_source::LineCol::new(1, 0)
            );
        }

        #[test]
        fn test_update_document_updates_source() {
            let mut workspace = Workspace::new();
            let mut db = TestDb::new(workspace.file_system());

            let temp_dir = tempdir().unwrap();
            let file_path = Utf8PathBuf::from_path_buf(temp_dir.path().join("buffer.py")).unwrap();
            std::fs::write(file_path.as_std_path(), "disk").unwrap();
            let url = Url::from_file_path(file_path.as_std_path()).unwrap();

            let document = TextDocument::new("initial".to_string(), 1, LanguageId::Python);
            let event = workspace.open_document(&mut db, &url, document).unwrap();
            let file = event.file();

            let changes = vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "updated".to_string(),
            }];
            workspace
                .update_document(&mut db, &url, changes, 2, PositionEncoding::Utf16)
                .unwrap();

            let source = file.source(&db);
            assert_eq!(source.as_str(), "updated");
        }

        #[test]
        fn test_close_document_reverts_to_disk() {
            let mut workspace = Workspace::new();
            let mut db = TestDb::new(workspace.file_system());

            let temp_dir = tempdir().unwrap();
            let file_path = Utf8PathBuf::from_path_buf(temp_dir.path().join("close.py")).unwrap();
            std::fs::write(file_path.as_std_path(), "disk content").unwrap();
            let url = Url::from_file_path(file_path.as_std_path()).unwrap();

            let document = TextDocument::new("buffer content".to_string(), 1, LanguageId::Python);
            let event = workspace.open_document(&mut db, &url, document).unwrap();
            let file = event.file();

            assert_eq!(file.source(&db).as_str(), "buffer content");

            workspace.close_document(&mut db, &url);

            let source_after = file.source(&db);
            assert_eq!(source_after.as_str(), "disk content");
        }
    }
}

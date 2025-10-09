//! Workspace facade for managing buffer and file system components
//!
//! This module provides the [`Workspace`] struct that encapsulates buffer
//! management and the virtual file system overlay shared with the Salsa
//! database. The database itself stays pure—[`Workspace`] surfaces the mutable
//! state (open documents) while the database observes it through the overlay.
use std::sync::Arc;

use camino::Utf8Path;
use djls_source::FileKind;
use djls_source::PositionEncoding;

use crate::db::Db;
use crate::document::DocumentChange;
use crate::document::TextDocument;
use crate::files::Buffers;
use crate::files::FileSystem;
use crate::files::OsFileSystem;
use crate::files::OverlayFileSystem;

/// Workspace facade that manages buffers and file system.
///
/// This struct provides a unified interface for managing document buffers
/// and file system operations. The Salsa database is managed at a higher
/// level (Session) and passed in when needed for operations.
pub struct Workspace {
    /// Thread-safe shared buffer storage for open documents
    buffers: Buffers,
    /// File system abstraction that checks buffers first, then disk
    overlay: Arc<OverlayFileSystem>,
}

impl Workspace {
    /// Create a new [`Workspace`] with buffers and file system initialized.
    #[must_use]
    pub fn new() -> Self {
        let buffers = Buffers::new();
        let overlay = Arc::new(OverlayFileSystem::new(
            buffers.clone(),
            Arc::new(OsFileSystem),
        ));

        Self { buffers, overlay }
    }

    /// Get the overlay file system for this workspace.
    ///
    /// The overlay returns buffer contents when present and falls back to disk
    /// otherwise.
    #[must_use]
    pub fn overlay(&self) -> Arc<dyn FileSystem> {
        self.overlay.clone()
    }

    /// Get the buffers for direct access.
    #[must_use]
    pub fn buffers(&self) -> &Buffers {
        &self.buffers
    }

    /// Get a document from the buffer if it's open.
    #[must_use]
    pub fn get_document(&self, path: &Utf8Path) -> Option<TextDocument> {
        self.buffers.get(path)
    }

    /// Open a document in the workspace and ensure a corresponding Salsa file exists.
    pub fn open_document(
        &mut self,
        db: &mut dyn Db,
        path: &Utf8Path,
        content: &str,
        version: i32,
        kind: FileKind,
    ) -> Option<TextDocument> {
        let file = db.get_or_create_file(path);
        let document = TextDocument::new(content.to_string(), version, kind, file);
        db.bump_file_revision(document.file());
        self.buffers.open(path.to_path_buf(), document.clone());
        Some(document)
    }

    pub fn save_document(&mut self, db: &mut dyn Db, path: &Utf8Path) -> Option<TextDocument> {
        let document = self.buffers.get(path)?;
        db.bump_file_revision(document.file());
        Some(document)
    }

    pub fn update_document(
        &mut self,
        db: &mut dyn Db,
        path: &Utf8Path,
        changes: Vec<DocumentChange>,
        version: i32,
        encoding: PositionEncoding,
    ) -> Option<TextDocument> {
        if let Some(mut document) = self.buffers.get(path) {
            db.bump_file_revision(document.file());
            document.update(changes, version, encoding);
            self.buffers.update(path.to_path_buf(), document.clone());
            Some(document)
        } else if let Some(first_change) = changes.into_iter().next() {
            if first_change.range().is_none() {
                let file = db.get_or_create_file(path);
                let document = TextDocument::new(
                    first_change.text().to_string(),
                    version,
                    FileKind::Other,
                    file,
                );
                self.buffers.open(path.to_path_buf(), document.clone());
                Some(document)
            } else {
                None
            }
        } else {
            None
        }
    }

    /// Close a document, removing it from buffers and touching the tracked file.
    pub fn close_document(&mut self, db: &mut dyn Db, path: &Utf8Path) -> Option<TextDocument> {
        let document = self.buffers.close(path)?;
        db.bump_file_revision(document.file());
        Some(document)
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
        use std::io;

        use camino::Utf8Path;
        use camino::Utf8PathBuf;

        use super::*;
        use crate::files::InMemoryFileSystem;

        #[salsa::db]
        #[derive(Clone)]
        struct TestDb {
            storage: salsa::Storage<Self>,
        }

        impl Default for TestDb {
            fn default() -> Self {
                Self {
                    storage: salsa::Storage::new(None),
                }
            }
        }

        #[salsa::db]
        impl salsa::Database for TestDb {}

        #[salsa::db]
        impl djls_source::Db for TestDb {
            fn create_file(&self, path: &Utf8Path) -> djls_source::File {
                djls_source::File::new(self, path.to_owned(), 0)
            }

            fn get_file(&self, _path: &Utf8Path) -> Option<djls_source::File> {
                None
            }

            fn read_file(&self, _path: &Utf8Path) -> std::io::Result<String> {
                Ok(String::new())
            }
        }

        fn text_document(content: &str, version: i32, kind: FileKind) -> TextDocument {
            let db = TestDb::default();
            let path = Utf8Path::new("/test.txt");
            let file = djls_source::File::new(&db, path.into(), 0);
            TextDocument::new(content.to_string(), version, kind, file)
        }

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
            let fs = OverlayFileSystem::new(buffers.clone(), disk);

            // Add file to buffer
            let path = test_file_path("test.py");
            let doc = text_document("buffer content", 1, FileKind::Python);
            buffers.open(path.clone(), doc);

            assert_eq!(fs.read_to_string(&path).unwrap(), "buffer content");
        }

        #[test]
        fn test_reads_from_disk_when_no_buffer() {
            let mut disk_fs = InMemoryFileSystem::new();
            let path = test_file_path("test.py");
            disk_fs.add_file(path.clone(), "disk content".to_string());

            let buffers = Buffers::new();
            let fs = OverlayFileSystem::new(buffers, Arc::new(disk_fs));

            assert_eq!(fs.read_to_string(&path).unwrap(), "disk content");
        }

        #[test]
        fn test_buffer_overrides_disk() {
            let mut disk_fs = InMemoryFileSystem::new();
            let path = test_file_path("test.py");
            disk_fs.add_file(path.clone(), "disk content".to_string());

            let buffers = Buffers::new();
            let fs = OverlayFileSystem::new(buffers.clone(), Arc::new(disk_fs));

            // Add buffer with different content
            let doc = text_document("buffer content", 1, FileKind::Python);
            buffers.open(path.clone(), doc);

            assert_eq!(fs.read_to_string(&path).unwrap(), "buffer content");
        }

        #[test]
        fn test_exists_for_buffer_only_file() {
            let disk = Arc::new(InMemoryFileSystem::new());
            let buffers = Buffers::new();
            let fs = OverlayFileSystem::new(buffers.clone(), disk);

            // Add file to buffer only
            let path = test_file_path("buffer_only.py");
            let doc = text_document("content", 1, FileKind::Python);
            buffers.open(path.clone(), doc);

            assert!(fs.exists(&path));
        }

        #[test]
        fn test_exists_for_disk_only_file() {
            let mut disk_fs = InMemoryFileSystem::new();
            let path = test_file_path("disk_only.py");
            disk_fs.add_file(path.clone(), "content".to_string());

            let buffers = Buffers::new();
            let fs = OverlayFileSystem::new(buffers, Arc::new(disk_fs));

            assert!(fs.exists(&path));
        }

        #[test]
        fn test_exists_for_both_buffer_and_disk() {
            let mut disk_fs = InMemoryFileSystem::new();
            let path = test_file_path("both.py");
            disk_fs.add_file(path.clone(), "disk".to_string());

            let buffers = Buffers::new();
            let fs = OverlayFileSystem::new(buffers.clone(), Arc::new(disk_fs));

            // Also add to buffer
            let doc = text_document("buffer", 1, FileKind::Python);
            buffers.open(path.clone(), doc);

            assert!(fs.exists(&path));
        }

        #[test]
        fn test_exists_returns_false_when_nowhere() {
            let disk = Arc::new(InMemoryFileSystem::new());
            let buffers = Buffers::new();
            let fs = OverlayFileSystem::new(buffers, disk);

            let path = test_file_path("nowhere.py");
            assert!(!fs.exists(&path));
        }

        #[test]
        fn test_read_error_when_file_nowhere() {
            let disk = Arc::new(InMemoryFileSystem::new());
            let buffers = Buffers::new();
            let fs = OverlayFileSystem::new(buffers, disk);

            let path = test_file_path("missing.py");
            let result = fs.read_to_string(&path);
            assert!(result.is_err());
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotFound);
        }

        #[test]
        fn test_reflects_buffer_updates() {
            let disk = Arc::new(InMemoryFileSystem::new());
            let buffers = Buffers::new();
            let fs = OverlayFileSystem::new(buffers.clone(), disk);

            let path = test_file_path("test.py");

            // Initial buffer content
            let doc1 = text_document("version 1", 1, FileKind::Python);
            buffers.open(path.clone().clone(), doc1);
            assert_eq!(fs.read_to_string(&path).unwrap(), "version 1");

            // Update buffer content
            let doc2 = text_document("version 2", 2, FileKind::Python);
            buffers.update(path.clone(), doc2);
            assert_eq!(fs.read_to_string(&path).unwrap(), "version 2");
        }

        #[test]
        fn test_handles_buffer_removal() {
            let mut disk_fs = InMemoryFileSystem::new();
            let path = test_file_path("test.py");
            disk_fs.add_file(path.clone(), "disk content".to_string());

            let buffers = Buffers::new();
            let fs = OverlayFileSystem::new(buffers.clone(), Arc::new(disk_fs));

            // Add buffer
            let doc = text_document("buffer content", 1, FileKind::Python);
            buffers.open(path.clone().clone(), doc);
            assert_eq!(fs.read_to_string(&path).unwrap(), "buffer content");

            // Remove buffer
            let _ = buffers.close(&path);
            assert_eq!(fs.read_to_string(&path).unwrap(), "disk content");
        }
    }

    mod workspace {
        use std::sync::Arc;

        use camino::Utf8Path;
        use camino::Utf8PathBuf;
        use djls_source::File;
        use djls_source::FxDashMap;
        use tempfile::tempdir;

        use super::*;

        #[salsa::db]
        #[derive(Clone)]
        struct TestDb {
            storage: salsa::Storage<Self>,
            fs: Arc<dyn FileSystem>,
            files: Arc<FxDashMap<Utf8PathBuf, File>>,
        }

        impl TestDb {
            fn new(fs: Arc<dyn FileSystem>) -> Self {
                Self {
                    storage: salsa::Storage::default(),
                    fs,
                    files: Arc::new(FxDashMap::default()),
                }
            }
        }

        #[salsa::db]
        impl salsa::Database for TestDb {}

        #[salsa::db]
        impl djls_source::Db for TestDb {
            fn create_file(&self, path: &Utf8Path) -> File {
                let file = File::new(self, path.to_owned(), 0);
                self.files.insert(path.to_owned(), file);
                file
            }

            fn get_file(&self, path: &Utf8Path) -> Option<File> {
                self.files.get(path).map(|entry| *entry)
            }

            fn read_file(&self, path: &Utf8Path) -> std::io::Result<String> {
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
            let mut db = TestDb::new(workspace.overlay());
            let path = Utf8Path::new("/test.py");

            let document = workspace
                .open_document(&mut db, path, "print('hello')", 1, FileKind::Python)
                .unwrap();
            let file_path = document.file().path(&db);
            assert_eq!(file_path.file_name(), Some("test.py"));
            assert!(workspace.buffers.get(path).is_some());
        }

        #[test]
        fn test_update_document() {
            let mut workspace = Workspace::new();
            let mut db = TestDb::new(workspace.overlay());
            let path = Utf8Path::new("/test.py");
            workspace.open_document(&mut db, path, "initial", 1, FileKind::Python);

            let changes = vec![crate::document::DocumentChange::new(
                None,
                "updated".to_string(),
            )];
            let document = workspace
                .update_document(&mut db, path, changes, 2, PositionEncoding::Utf16)
                .unwrap();

            assert_eq!(document.file().path(&db).file_name(), Some("test.py"));
            let buffer = workspace.buffers.get(path).unwrap();
            assert_eq!(buffer.content(), "updated");
            assert_eq!(buffer.version(), 2);
        }

        #[test]
        fn test_close_document() {
            let mut workspace = Workspace::new();
            let mut db = TestDb::new(workspace.overlay());
            let path = Utf8Path::new("/test.py");
            workspace.open_document(&mut db, path, "content", 1, FileKind::Python);

            let closed = workspace.close_document(&mut db, path);
            assert!(closed.is_some());
            assert!(workspace.buffers.get(path).is_none());
        }

        #[test]
        fn test_file_system_checks_buffers_first() {
            let temp_dir = tempdir().unwrap();
            let file_path = temp_dir.path().join("test.py");
            std::fs::write(&file_path, "disk content").unwrap();

            let mut workspace = Workspace::new();
            let mut db = TestDb::new(workspace.overlay());
            let path = Utf8Path::from_path(&file_path).unwrap();

            workspace.open_document(&mut db, path, "buffer content", 1, FileKind::Python);

            let content = workspace
                .overlay()
                .read_to_string(Utf8Path::from_path(&file_path).unwrap())
                .unwrap();
            assert_eq!(content, "buffer content");
        }

        #[test]
        fn test_file_source_reads_from_buffer() {
            let mut workspace = Workspace::new();
            let mut db = TestDb::new(workspace.overlay());

            let temp_dir = tempdir().unwrap();
            let file_path =
                Utf8PathBuf::from_path_buf(temp_dir.path().join("template.html")).unwrap();
            std::fs::write(file_path.as_std_path(), "disk template").unwrap();
            let document = workspace
                .open_document(&mut db, &file_path, "line1\nline2", 1, FileKind::Template)
                .unwrap();

            let source = document.file().source(&db);
            assert_eq!(source.as_str(), document.content());

            let line_index = document.file().line_index(&db);
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
            let mut db = TestDb::new(workspace.overlay());

            let temp_dir = tempdir().unwrap();
            let file_path = Utf8PathBuf::from_path_buf(temp_dir.path().join("buffer.py")).unwrap();
            std::fs::write(file_path.as_std_path(), "disk").unwrap();
            let document = workspace
                .open_document(&mut db, &file_path, "initial", 1, FileKind::Python)
                .unwrap();

            let changes = vec![crate::document::DocumentChange::new(
                None,
                "updated".to_string(),
            )];
            workspace
                .update_document(&mut db, &file_path, changes, 2, PositionEncoding::Utf16)
                .unwrap();

            let source = document.file().source(&db);
            assert_eq!(source.as_str(), "updated");
        }

        #[test]
        fn test_close_document_reverts_to_disk() {
            let mut workspace = Workspace::new();
            let mut db = TestDb::new(workspace.overlay());

            let temp_dir = tempdir().unwrap();
            let file_path = Utf8PathBuf::from_path_buf(temp_dir.path().join("close.py")).unwrap();
            std::fs::write(file_path.as_std_path(), "disk content").unwrap();
            let document = workspace
                .open_document(&mut db, &file_path, "buffer content", 1, FileKind::Python)
                .unwrap();

            let file = document.file();
            assert_eq!(file.source(&db).as_str(), "buffer content");

            workspace.close_document(&mut db, &file_path);

            let source_after = file.source(&db);
            assert_eq!(source_after.as_str(), "disk content");
        }
    }
}

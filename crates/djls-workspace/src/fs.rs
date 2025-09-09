//! Virtual file system abstraction
//!
//! This module provides the [`FileSystem`] trait that abstracts file I/O operations.
//! This allows the LSP to work with both real files and in-memory overlays.

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use crate::buffers::Buffers;
use crate::paths;

pub trait FileSystem: Send + Sync {
    fn read_to_string(&self, path: &Path) -> io::Result<String>;
    fn exists(&self, path: &Path) -> bool;
}

pub struct InMemoryFileSystem {
    files: HashMap<PathBuf, String>,
}

impl InMemoryFileSystem {
    #[must_use]
    pub fn new() -> Self {
        Self {
            files: HashMap::new(),
        }
    }

    pub fn add_file(&mut self, path: PathBuf, content: String) {
        self.files.insert(path, content);
    }
}

impl Default for InMemoryFileSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl FileSystem for InMemoryFileSystem {
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "File not found"))
    }

    fn exists(&self, path: &Path) -> bool {
        self.files.contains_key(path)
    }
}

/// Standard file system implementation that uses [`std::fs`].
pub struct OsFileSystem;

impl FileSystem for OsFileSystem {
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
}

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
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        if let Some(url) = paths::path_to_url(path) {
            if let Some(document) = self.buffers.get(&url) {
                return Ok(document.content().to_string());
            }
        }
        self.disk.read_to_string(path)
    }

    fn exists(&self, path: &Path) -> bool {
        paths::path_to_url(path).is_some_and(|url| self.buffers.contains(&url))
            || self.disk.exists(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod in_memory {
        use super::*;

        #[test]
        fn test_read_existing_file() {
            let mut fs = InMemoryFileSystem::new();
            fs.add_file("/test.py".into(), "file content".to_string());

            assert_eq!(
                fs.read_to_string(Path::new("/test.py")).unwrap(),
                "file content"
            );
        }

        #[test]
        fn test_read_nonexistent_file() {
            let fs = InMemoryFileSystem::new();

            let result = fs.read_to_string(Path::new("/missing.py"));
            assert!(result.is_err());
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotFound);
        }

        #[test]
        fn test_exists_returns_true_for_existing() {
            let mut fs = InMemoryFileSystem::new();
            fs.add_file("/exists.py".into(), "content".to_string());

            assert!(fs.exists(Path::new("/exists.py")));
        }

        #[test]
        fn test_exists_returns_false_for_nonexistent() {
            let fs = InMemoryFileSystem::new();

            assert!(!fs.exists(Path::new("/missing.py")));
        }
    }

    mod workspace {
        use url::Url;

        use super::*;
        use crate::buffers::Buffers;
        use crate::document::TextDocument;
        use crate::language::LanguageId;

        // Helper to create platform-appropriate test paths
        fn test_file_path(name: &str) -> PathBuf {
            #[cfg(windows)]
            return PathBuf::from(format!("C:\\temp\\{name}"));
            #[cfg(not(windows))]
            return PathBuf::from(format!("/tmp/{name}"));
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
}

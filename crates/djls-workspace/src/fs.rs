//! File system abstraction following Ruff's pattern
//!
//! This module provides the `FileSystem` trait that abstracts file I/O operations.
//! This allows the LSP to work with both real files and in-memory overlays.

use std::io;
use std::path::Path;
use std::sync::Arc;
use url::Url;

use crate::buffers::Buffers;

/// Trait for file system operations
///
/// This follows Ruff's pattern of abstracting file system operations behind a trait,
/// allowing different implementations for testing, in-memory operation, and real file access.
pub trait FileSystem: Send + Sync {
    /// Read the entire contents of a file
    fn read_to_string(&self, path: &Path) -> io::Result<String>;

    /// Check if a path exists
    fn exists(&self, path: &Path) -> bool;

    /// Check if a path is a file
    fn is_file(&self, path: &Path) -> bool;

    /// Check if a path is a directory
    fn is_directory(&self, path: &Path) -> bool;

    /// List directory contents
    fn read_directory(&self, path: &Path) -> io::Result<Vec<std::path::PathBuf>>;

    /// Get file metadata (size, modified time, etc.)
    fn metadata(&self, path: &Path) -> io::Result<std::fs::Metadata>;
}

/// Standard file system implementation that uses `std::fs`
pub struct OsFileSystem;

impl FileSystem for OsFileSystem {
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn is_directory(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn read_directory(&self, path: &Path) -> io::Result<Vec<std::path::PathBuf>> {
        std::fs::read_dir(path)?
            .map(|entry| entry.map(|e| e.path()))
            .collect()
    }

    fn metadata(&self, path: &Path) -> io::Result<std::fs::Metadata> {
        std::fs::metadata(path)
    }
}

/// LSP file system that intercepts reads for buffered files
///
/// This implements Ruff's two-layer architecture where Layer 1 (open buffers)
/// takes precedence over Layer 2 (Salsa database). When a file is read,
/// this system first checks for a buffer (in-memory content) and returns
/// that content. If no buffer exists, it falls back to reading from disk.
pub struct WorkspaceFileSystem {
    /// In-memory buffers that take precedence over disk files
    buffers: Buffers,
    /// Fallback file system for disk operations
    disk: Arc<dyn FileSystem>,
}

impl WorkspaceFileSystem {
    /// Create a new [`WorkspaceFileSystem`] with the given buffer storage and fallback
    pub fn new(buffers: Buffers, disk: Arc<dyn FileSystem>) -> Self {
        Self { buffers, disk }
    }
}

impl FileSystem for WorkspaceFileSystem {
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        if let Some(url) = path_to_url(path) {
            if let Some(document) = self.buffers.get(&url) {
                return Ok(document.content().to_string());
            }
        }
        self.disk.read_to_string(path)
    }

    fn exists(&self, path: &Path) -> bool {
        path_to_url(path).is_some_and(|url| self.buffers.contains(&url))
            || self.disk.exists(path)
    }

    fn is_file(&self, path: &Path) -> bool {
        path_to_url(path).is_some_and(|url| self.buffers.contains(&url))
            || self.disk.is_file(path)
    }

    fn is_directory(&self, path: &Path) -> bool {
        // Overlays are never directories, so just delegate
        self.disk.is_directory(path)
    }

    fn read_directory(&self, path: &Path) -> io::Result<Vec<std::path::PathBuf>> {
        // Overlays are never directories, so just delegate
        self.disk.read_directory(path)
    }

    fn metadata(&self, path: &Path) -> io::Result<std::fs::Metadata> {
        // For overlays, we could synthesize metadata, but for simplicity,
        // fall back to disk. This might need refinement for edge cases.
        self.disk.metadata(path)
    }
}

/// Convert a file path to URL for overlay lookup
///
/// This is a simplified conversion - in a full implementation,
/// you might want more robust path-to-URL conversion
fn path_to_url(path: &Path) -> Option<Url> {
    // For absolute paths, use them directly without canonicalization
    // This ensures consistency with how URLs are created when storing overlays
    if path.is_absolute() {
        return Url::from_file_path(path).ok();
    }

    // Only try to canonicalize for relative paths
    if let Ok(absolute_path) = std::fs::canonicalize(path) {
        return Url::from_file_path(absolute_path).ok();
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffers::Buffers;
    use crate::document::TextDocument;
    use crate::language::LanguageId;

    /// In-memory file system for testing
    pub struct InMemoryFileSystem {
        files: std::collections::HashMap<std::path::PathBuf, String>,
    }

    impl InMemoryFileSystem {
        pub fn new() -> Self {
            Self {
                files: std::collections::HashMap::new(),
            }
        }

        pub fn add_file(&mut self, path: std::path::PathBuf, content: String) {
            self.files.insert(path, content);
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

        fn is_file(&self, path: &Path) -> bool {
            self.files.contains_key(path)
        }

        fn is_directory(&self, _path: &Path) -> bool {
            // Simplified for testing - no directories in memory filesystem
            false
        }

        fn read_directory(&self, _path: &Path) -> io::Result<Vec<std::path::PathBuf>> {
            // Simplified for testing
            Ok(Vec::new())
        }

        fn metadata(&self, _path: &Path) -> io::Result<std::fs::Metadata> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Metadata not supported in memory filesystem",
            ))
        }
    }

    #[test]
    fn test_lsp_filesystem_overlay_precedence() {
        // Create a memory filesystem with some content
        let mut memory_fs = InMemoryFileSystem::new();
        memory_fs.add_file(
            std::path::PathBuf::from("/test/file.py"),
            "original content".to_string(),
        );

        // Create buffer storage
        let buffers = Buffers::new();

        // Create LspFileSystem with memory fallback
        let lsp_fs = WorkspaceFileSystem::new(buffers.clone(), Arc::new(memory_fs));

        // Before adding buffer, should read from fallback
        let path = std::path::Path::new("/test/file.py");
        assert_eq!(lsp_fs.read_to_string(path).unwrap(), "original content");

        // Add buffer - this simulates having an open document with changes
        let url = Url::from_file_path("/test/file.py").unwrap();
        let document = TextDocument::new("overlay content".to_string(), 1, LanguageId::Python);
        buffers.open(url, document);

        // Now should read from buffer
        assert_eq!(lsp_fs.read_to_string(path).unwrap(), "overlay content");
    }

    #[test]
    fn test_lsp_filesystem_fallback_when_no_overlay() {
        // Create memory filesystem
        let mut memory_fs = InMemoryFileSystem::new();
        memory_fs.add_file(
            std::path::PathBuf::from("/test/file.py"),
            "disk content".to_string(),
        );

        // Create empty buffer storage
        let buffers = Buffers::new();

        // Create LspFileSystem
        let lsp_fs = WorkspaceFileSystem::new(buffers, Arc::new(memory_fs));

        // Should fall back to disk when no buffer exists
        let path = std::path::Path::new("/test/file.py");
        assert_eq!(lsp_fs.read_to_string(path).unwrap(), "disk content");
    }

    #[test]
    fn test_lsp_filesystem_other_operations_delegate() {
        // Create memory filesystem
        let mut memory_fs = InMemoryFileSystem::new();
        memory_fs.add_file(
            std::path::PathBuf::from("/test/file.py"),
            "content".to_string(),
        );

        // Create LspFileSystem
        let buffers = Buffers::new();
        let lsp_fs = WorkspaceFileSystem::new(buffers, Arc::new(memory_fs));

        let path = std::path::Path::new("/test/file.py");

        // These should delegate to the fallback filesystem
        assert!(lsp_fs.exists(path));
        assert!(lsp_fs.is_file(path));
        assert!(!lsp_fs.is_directory(path));
    }
}

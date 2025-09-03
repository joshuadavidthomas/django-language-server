//! Virtual file system abstraction
//!
//! This module provides the [`FileSystem`] trait that abstracts file I/O operations.
//! This allows the LSP to work with both real files and in-memory overlays.

#[cfg(test)]
use std::collections::HashMap;
use std::io;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::sync::Arc;

use crate::buffers::Buffers;
use crate::paths;

/// Trait for file system operations
pub trait FileSystem: Send + Sync {
    /// Read the entire contents of a file
    fn read_to_string(&self, path: &Path) -> io::Result<String>;

    /// Check if a path exists
    fn exists(&self, path: &Path) -> bool;
}

/// In-memory file system for testing
#[cfg(test)]
pub struct InMemoryFileSystem {
    files: HashMap<PathBuf, String>,
}

#[cfg(test)]
impl InMemoryFileSystem {
    pub fn new() -> Self {
        Self {
            files: HashMap::new(),
        }
    }

    pub fn add_file(&mut self, path: PathBuf, content: String) {
        self.files.insert(path, content);
    }
}

#[cfg(test)]
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
/// This type is used by the [`Database`](crate::db::Database) to ensure all file reads go
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
    use url::Url;

    use super::*;
    use crate::buffers::Buffers;
    use crate::document::TextDocument;
    use crate::language::LanguageId;

    #[test]
    fn test_lsp_filesystem_overlay_precedence() {
        let mut memory_fs = InMemoryFileSystem::new();
        memory_fs.add_file(
            std::path::PathBuf::from("/test/file.py"),
            "original content".to_string(),
        );

        let buffers = Buffers::new();
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
        let mut memory_fs = InMemoryFileSystem::new();
        memory_fs.add_file(
            std::path::PathBuf::from("/test/file.py"),
            "disk content".to_string(),
        );

        let buffers = Buffers::new();
        let lsp_fs = WorkspaceFileSystem::new(buffers, Arc::new(memory_fs));

        // Should fall back to disk when no buffer exists
        let path = std::path::Path::new("/test/file.py");
        assert_eq!(lsp_fs.read_to_string(path).unwrap(), "disk content");
    }

    #[test]
    fn test_lsp_filesystem_other_operations_delegate() {
        let mut memory_fs = InMemoryFileSystem::new();
        memory_fs.add_file(
            std::path::PathBuf::from("/test/file.py"),
            "content".to_string(),
        );

        let buffers = Buffers::new();
        let lsp_fs = WorkspaceFileSystem::new(buffers, Arc::new(memory_fs));

        let path = std::path::Path::new("/test/file.py");

        // This should delegate to the fallback filesystem
        assert!(lsp_fs.exists(path));
    }

    #[test]
    fn test_overlay_consistency() {
        // Create an empty filesystem (no files on disk)
        let memory_fs = InMemoryFileSystem::new();
        let buffers = Buffers::new();
        let lsp_fs = WorkspaceFileSystem::new(buffers.clone(), Arc::new(memory_fs));

        let path = std::path::Path::new("/test/overlay_only.py");

        // Before adding to overlay, file doesn't exist
        assert!(!lsp_fs.exists(path));

        // Add file to overlay only (not on disk)
        let url = Url::from_file_path("/test/overlay_only.py").unwrap();
        let document = TextDocument::new("overlay content".to_string(), 1, LanguageId::Python);
        buffers.open(url, document);

        // Now file should exist
        assert!(lsp_fs.exists(path), "Overlay file should exist");

        // And we should be able to read its content
        assert_eq!(
            lsp_fs.read_to_string(path).unwrap(),
            "overlay content",
            "Should read overlay content"
        );
    }

    #[test]
    fn test_overlay_with_relative_path() {
        // Create an empty filesystem (no files on disk)
        let memory_fs = InMemoryFileSystem::new();
        let buffers = Buffers::new();
        let lsp_fs = WorkspaceFileSystem::new(buffers.clone(), Arc::new(memory_fs));

        // Use a relative path that doesn't exist on disk
        let relative_path = std::path::Path::new("nonexistent/overlay.py");

        // Convert to absolute URL for the buffer (simulating how LSP would provide it)
        let absolute_path = std::env::current_dir().unwrap().join(relative_path);
        let url = Url::from_file_path(&absolute_path).unwrap();

        // Add to overlay
        let document = TextDocument::new("relative overlay".to_string(), 1, LanguageId::Python);
        buffers.open(url, document);

        // The relative path should now work through the overlay
        assert!(
            lsp_fs.exists(relative_path),
            "Relative overlay file should exist"
        );
        assert_eq!(
            lsp_fs.read_to_string(relative_path).unwrap(),
            "relative overlay",
            "Should read relative overlay content"
        );
    }
}

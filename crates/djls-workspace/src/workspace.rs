//! Workspace facade for managing buffer and file system components
//!
//! This module provides the [`Workspace`] struct that encapsulates buffer
//! management and file system abstraction. The Salsa database is managed
//! at the Session level, following Ruff's architecture pattern.

use std::sync::Arc;

use tower_lsp_server::lsp_types::TextDocumentContentChangeEvent;
use url::Url;

use crate::buffers::Buffers;
use crate::document::TextDocument;
use crate::fs::FileSystem;
use crate::fs::OsFileSystem;
use crate::fs::WorkspaceFileSystem;

/// Workspace facade that manages buffers and file system.
///
/// This struct provides a unified interface for managing document buffers
/// and file system operations. The Salsa database is managed at a higher
/// level (Session) and passed in when needed for operations.
pub struct Workspace {
    /// Thread-safe shared buffer storage for open documents
    buffers: Buffers,
    /// File system abstraction that checks buffers first, then disk
    file_system: Arc<WorkspaceFileSystem>,
}

impl Workspace {
    /// Create a new [`Workspace`] with buffers and file system initialized.
    #[must_use]
    pub fn new() -> Self {
        let buffers = Buffers::new();
        let file_system = Arc::new(WorkspaceFileSystem::new(
            buffers.clone(),
            Arc::new(OsFileSystem),
        ));

        Self {
            buffers,
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

    /// Open a document in the workspace.
    ///
    /// Adds the document to the buffer layer. The database should be
    /// notified separately by the caller if invalidation is needed.
    pub fn open_document(&mut self, url: &Url, document: TextDocument) {
        self.buffers.open(url.clone(), document);
    }

    /// Update a document with incremental changes.
    ///
    /// Applies changes to the existing document in buffers.
    /// Falls back to full replacement if the document isn't currently open.
    pub fn update_document(
        &mut self,
        url: &Url,
        changes: Vec<TextDocumentContentChangeEvent>,
        version: i32,
        encoding: crate::encoding::PositionEncoding,
    ) {
        if let Some(mut document) = self.buffers.get(url) {
            // Apply incremental changes to existing document
            document.update(changes, version, encoding);
            self.buffers.update(url.clone(), document);
        } else if let Some(first_change) = changes.into_iter().next() {
            // Fallback: treat first change as full replacement
            if first_change.range.is_none() {
                let document = TextDocument::new(
                    first_change.text,
                    version,
                    crate::language::LanguageId::Other,
                );
                self.buffers.open(url.clone(), document);
            }
        }
    }

    /// Close a document and return it.
    ///
    /// Removes from buffers. The database should be notified
    /// separately by the caller if invalidation is needed.
    pub fn close_document(&mut self, url: &Url) -> Option<TextDocument> {
        self.buffers.close(url)
    }

    /// Get a document from the buffer if it's open.
    ///
    /// Returns a cloned [`TextDocument`] for the given URL if it exists in buffers.
    #[must_use]
    pub fn get_document(&self, url: &Url) -> Option<TextDocument> {
        self.buffers.get(url)
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::encoding::PositionEncoding;
    use crate::LanguageId;

    #[test]
    fn test_open_document() {
        let mut workspace = Workspace::new();
        let url = Url::parse("file:///test.py").unwrap();

        // Open document
        let document = TextDocument::new("print('hello')".to_string(), 1, LanguageId::Python);
        workspace.open_document(&url, document);

        // Should be in buffers
        assert!(workspace.buffers.get(&url).is_some());
    }

    #[test]
    fn test_update_document() {
        let mut workspace = Workspace::new();
        let url = Url::parse("file:///test.py").unwrap();

        // Open with initial content
        let document = TextDocument::new("initial".to_string(), 1, LanguageId::Python);
        workspace.open_document(&url, document);

        // Update content
        let changes = vec![TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "updated".to_string(),
        }];
        workspace.update_document(&url, changes, 2, PositionEncoding::Utf16);

        // Verify buffer was updated
        let buffer = workspace.buffers.get(&url).unwrap();
        assert_eq!(buffer.content(), "updated");
        assert_eq!(buffer.version(), 2);
    }

    #[test]
    fn test_close_document() {
        let mut workspace = Workspace::new();
        let url = Url::parse("file:///test.py").unwrap();

        // Open document
        let document = TextDocument::new("content".to_string(), 1, LanguageId::Python);
        workspace.open_document(&url, document.clone());

        // Close it
        let closed = workspace.close_document(&url);
        assert!(closed.is_some());

        // Should no longer be in buffers
        assert!(workspace.buffers.get(&url).is_none());
    }

    #[test]
    fn test_file_system_checks_buffers_first() {
        let temp_dir = tempdir().unwrap();
        let file_path = temp_dir.path().join("test.py");
        std::fs::write(&file_path, "disk content").unwrap();

        let mut workspace = Workspace::new();
        let url = Url::from_file_path(&file_path).unwrap();

        // Open document with different content than disk
        let document = TextDocument::new("buffer content".to_string(), 1, LanguageId::Python);
        workspace.open_document(&url, document);

        // File system should return buffer content, not disk content
        let content = workspace.file_system().read_to_string(&file_path).unwrap();
        assert_eq!(content, "buffer content");
    }
}

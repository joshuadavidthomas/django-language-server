//! Workspace management for the Django Language Server
//!
//! This crate provides the core workspace functionality including document management,
//! file system abstractions, and Salsa integration for incremental computation of
//! Django projects.
//!
//! # Key Components
//!
//! - [`Buffers`] - Thread-safe storage for open documents
//! - [`Db`] - Database trait for file system access (concrete impl in server crate)
//! - [`TextDocument`] - LSP document representation with efficient indexing
//! - [`FileSystem`] - Abstraction layer for file operations with overlay support
//! - [`paths`] - Consistent URL/path conversion utilities

mod buffers;
pub mod db;
mod document;
pub mod encoding;
mod fs;
mod language;
pub mod paths;
mod workspace;

use std::path::Path;

pub use buffers::Buffers;
pub use db::Db;
pub use db::SourceFile;
pub use document::TextDocument;
pub use encoding::PositionEncoding;
pub use fs::FileSystem;
pub use fs::InMemoryFileSystem;
pub use fs::OsFileSystem;
pub use fs::WorkspaceFileSystem;
pub use language::LanguageId;
pub use workspace::Workspace;

/// File classification for routing to analyzers.
///
/// [`FileKind`] determines how a file should be processed by downstream analyzers.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum FileKind {
    /// Python source file
    Python,
    /// Django template file
    Template,
    /// Other file type
    Other,
}

impl FileKind {
    /// Determine [`FileKind`] from a file path extension.
    #[must_use]
    pub fn from_path(path: &Path) -> Self {
        match path.extension().and_then(|s| s.to_str()) {
            Some("py") => FileKind::Python,
            Some("html" | "htm") => FileKind::Template,
            _ => FileKind::Other,
        }
    }
}

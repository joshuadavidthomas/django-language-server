mod buffers;
pub mod db;
mod document;
mod fs;
mod language;
mod template;

pub use buffers::Buffers;
pub use db::Database;
pub use document::TextDocument;
pub use fs::{FileSystem, OsFileSystem, WorkspaceFileSystem};
pub use language::LanguageId;

/// Stable, compact identifier for files across the subsystem.
///
/// [`FileId`] decouples file identity from paths/URIs, providing efficient keys for maps and
/// Salsa inputs. Once assigned to a file (via its URI), a [`FileId`] remains stable for the
/// lifetime of the system.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct FileId(u32);

impl FileId {
    /// Create a [`FileId`] from a raw u32 value.
    #[must_use]
    pub fn from_raw(raw: u32) -> Self {
        FileId(raw)
    }

    /// Get the underlying u32 index value.
    #[must_use]
    #[allow(dead_code)]
    pub fn index(self) -> u32 {
        self.0
    }
}

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
    /// Determine `FileKind` from a file path extension.
    #[must_use]
    pub fn from_path(path: &std::path::Path) -> Self {
        match path.extension().and_then(|s| s.to_str()) {
            Some("py") => FileKind::Python,
            Some("html" | "htm") => FileKind::Template,
            _ => FileKind::Other,
        }
    }
}

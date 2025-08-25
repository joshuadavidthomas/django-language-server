mod bridge;
mod db;
mod vfs;

// Re-export public API
pub use bridge::FileStore;
pub use db::{Database, FileKindMini, SourceFile, TemplateLoaderOrder};
pub use vfs::{FileKind, FileMeta, FileRecord, Revision, TextSource, Vfs, VfsSnapshot};

/// Stable, compact identifier for files across the subsystem.
///
/// [`FileId`] decouples file identity from paths/URIs, providing efficient keys for maps and
/// Salsa inputs. Once assigned to a file (via its URI), a [`FileId`] remains stable for the
/// lifetime of the VFS, even if the file's content or metadata changes.
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
    pub fn index(self) -> u32 {
        self.0
    }
}

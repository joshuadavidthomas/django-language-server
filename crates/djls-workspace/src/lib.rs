mod bridge;
mod db;
mod document;
mod vfs;

pub use document::ClosingBrace;
pub use document::DocumentStore;
pub use document::LanguageId;
pub use document::LineIndex;
pub use document::TemplateTagContext;
pub use document::TextDocument;

/// Stable, compact identifier for files across the subsystem.
///
/// [`FileId`] decouples file identity from paths/URIs, providing efficient keys for maps and
/// Salsa inputs. Once assigned to a file (via its URI), a [`FileId`] remains stable for the
/// lifetime of the VFS, even if the file's content or metadata changes.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub(crate) struct FileId(u32);

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

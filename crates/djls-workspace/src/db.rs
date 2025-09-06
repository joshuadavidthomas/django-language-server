//! Base database trait for workspace operations.
//!
//! This module provides the base [`Db`] trait that defines file system access
//! and core file tracking functionality. The concrete database implementation
//! lives in the server crate, following Ruff's architecture pattern.
//!
//! ## Architecture
//!
//! The system uses a layered trait approach:
//! 1. **Base trait** ([`Db`]) - Defines file system access methods (this module)
//! 2. **Extension traits** - Other crates (like djls-templates) extend this trait
//! 3. **Concrete implementation** - Server crate implements all traits
//!
//! ## The Revision Dependency
//!
//! The [`source_text`] function **must** call `file.revision(db)` to create
//! a Salsa dependency. Without this, revision changes won't invalidate queries:
//!
//! ```ignore
//! let _ = file.revision(db);  // Creates the dependency chain!
//! ```

use std::path::Path;
use std::sync::Arc;

use crate::FileKind;
use crate::FileSystem;

/// Base database trait that provides file system access for Salsa queries
#[salsa::db]
pub trait Db: salsa::Database {
    /// Get the file system for reading files.
    fn fs(&self) -> Arc<dyn FileSystem>;

    /// Read file content through the file system.
    ///
    /// Checks buffers first via [`WorkspaceFileSystem`](crate::fs::WorkspaceFileSystem),
    /// then falls back to disk.
    fn read_file_content(&self, path: &Path) -> std::io::Result<String>;
}

/// Represents a single file without storing its content.
///
/// [`SourceFile`] is a Salsa input entity that tracks a file's path, revision, and
/// classification for analysis routing. Following Ruff's pattern, content is NOT
/// stored here but read on-demand through the `source_text` tracked function.
#[salsa::input]
pub struct SourceFile {
    /// The file's classification for analysis routing
    pub kind: FileKind,
    /// The file path
    #[returns(ref)]
    pub path: Arc<str>,
    /// The revision number for invalidation tracking
    pub revision: u64,
}

/// Read file content, creating a Salsa dependency on the file's revision.
#[salsa::tracked]
pub fn source_text(db: &dyn Db, file: SourceFile) -> Arc<str> {
    // This line creates the Salsa dependency on revision! Without this call,
    // revision changes won't trigger invalidation
    let _ = file.revision(db);

    let path = Path::new(file.path(db).as_ref());
    match db.read_file_content(path) {
        Ok(content) => Arc::from(content),
        Err(_) => {
            Arc::from("") // Return empty string for missing files
        }
    }
}

/// Represents a file path for Salsa tracking.
///
/// [`FilePath`] is a Salsa input entity that tracks a file path for use in
/// path-based queries. This allows Salsa to properly track dependencies
/// on files identified by path rather than by SourceFile input.
#[salsa::input]
pub struct FilePath {
    /// The file path as a string
    #[returns(ref)]
    pub path: Arc<str>,
}

// Template-specific functionality has been moved to djls-templates crate
// See djls_templates::db for template parsing and diagnostics

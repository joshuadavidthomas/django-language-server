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

use std::sync::Arc;

use djls_source::Db as SourceDb;

use crate::FileSystem;

/// Base database trait that provides file system access for Salsa queries
#[salsa::db]
pub trait Db: SourceDb {
    /// Get the file system for reading files.
    fn fs(&self) -> Arc<dyn FileSystem>;
}

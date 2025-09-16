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

pub use buffers::Buffers;
pub use db::Db;
pub use document::TextDocument;
pub use encoding::PositionEncoding;
pub use fs::FileSystem;
pub use fs::InMemoryFileSystem;
pub use fs::OsFileSystem;
pub use fs::WorkspaceFileSystem;
pub use language::LanguageId;
pub use workspace::Workspace;
pub use workspace::WorkspaceFileEvent;

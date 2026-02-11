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

mod db;
mod document;
mod files;
mod walk;
mod workspace;

pub use db::Db;
pub use document::DocumentChange;
pub use document::TextDocument;
pub use files::FileSystem;
pub use files::InMemoryFileSystem;
pub use files::OsFileSystem;
pub use walk::walk_files;
pub use workspace::Workspace;

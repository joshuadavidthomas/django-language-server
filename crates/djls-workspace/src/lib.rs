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

mod db;
mod document;
mod files;
mod language;
pub mod paths;
mod workspace;

pub use db::Db;
pub use document::TextDocument;
pub use files::FileSystem;
pub use files::InMemoryFileSystem;
pub use language::LanguageId;
pub use workspace::Workspace;

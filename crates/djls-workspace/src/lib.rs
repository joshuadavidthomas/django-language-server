//! Workspace management for the Django Language Server
//!
//! This crate provides the core workspace functionality including document management,
//! file system abstractions, and workspace file discovery.
//!
//! # Key Components
//!
//! - [`TextDocument`] - LSP document representation with efficient indexing
//! - [`FileSystem`] - Abstraction layer for file operations with overlay support

mod document;
mod files;
mod walk;
mod workspace;

pub use document::DocumentChange;
pub use document::TextDocument;
pub use files::FileSystem;
pub use files::InMemoryFileSystem;
pub use files::OsFileSystem;
pub use walk::walk_files;
pub use walk::WalkOptions;
pub use workspace::Workspace;

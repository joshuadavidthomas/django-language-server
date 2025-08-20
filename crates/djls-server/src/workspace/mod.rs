//! # Workspace Management
//!
//! This module provides the core workspace functionality for the Django Language Server,
//! including file system abstraction, document management, and workspace utilities.
//!
//! ## Architecture
//!
//! The workspace module implements a custom VFS (Virtual File System) approach specifically
//! designed for LSP operations. This design decision was made to support proper handling
//! of unsaved editor changes while maintaining efficient access to disk-based files.
//!
//! ### Key Components
//!
//! - **[`FileSystem`]**: Custom VFS implementation with dual-layer architecture (memory + physical)
//! - **[`Store`]**: High-level document and workspace state management
//! - **Document types**: Structures for tracking document metadata and changes
//!
//! ### LSP Integration
//!
//! The workspace module is designed around LSP lifecycle events:
//! - `textDocument/didOpen`: Files are tracked but not immediately loaded into memory
//! - `textDocument/didChange`: Changes are stored in the memory layer
//! - `textDocument/didSave`: Memory layer changes can be discarded (editor handles disk writes)
//! - `textDocument/didClose`: Memory layer is cleaned up for the file
//!
//! This approach ensures that language server operations always see the most current
//! version of files (including unsaved changes) while preserving the original disk
//! state until the editor explicitly saves.
//!
//! ## Design Decisions
//!
//! See `backlog/decisions/decision-2` for the detailed rationale behind the custom
//! VFS implementation instead of using existing overlay filesystem libraries.

mod document;
mod store;
mod utils;
mod fs;

pub use store::Store;
pub use utils::get_project_path;
pub use fs::FileSystem;

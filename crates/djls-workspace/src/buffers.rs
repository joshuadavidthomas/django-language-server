//! Shared buffer storage for open documents
//!
//! This module provides the [`Buffers`] type which represents the in-memory
//! content of open files. These buffers are shared between the `Session`
//! (which manages document lifecycle) and the [`WorkspaceFileSystem`] (which
//! reads from them).
///
/// [`WorkspaceFileSystem`]: crate::fs::WorkspaceFileSystem
use std::sync::Arc;

use dashmap::DashMap;
use url::Url;

use crate::document::TextDocument;

/// Shared buffer storage between `Session` and [`FileSystem`].
///
/// Buffers represent the in-memory content of open files that takes
/// precedence over disk content when reading through the [`FileSystem`].
/// This is the key abstraction that makes the sharing between Session
/// and [`WorkspaceFileSystem`] explicit and type-safe.
///
/// The [`WorkspaceFileSystem`] holds a clone of this structure and checks
/// it before falling back to disk reads.
///
/// ## Memory Management
///
/// This structure does not implement eviction or memory limits because the
/// LSP protocol explicitly manages document lifecycle through `didOpen` and
/// `didClose` notifications. Documents are only stored while the editor has
/// them open, and are properly removed when the editor closes them. This
/// follows the battle-tested pattern used by production LSP servers like Ruff.
///
/// [`FileSystem`]: crate::fs::FileSystem
/// [`WorkspaceFileSystem`]: crate::fs::WorkspaceFileSystem
#[derive(Clone, Debug)]
pub struct Buffers {
    inner: Arc<DashMap<Url, TextDocument>>,
}

impl Buffers {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(DashMap::new()),
        }
    }

    pub fn open(&self, url: Url, document: TextDocument) {
        self.inner.insert(url, document);
    }

    pub fn update(&self, url: Url, document: TextDocument) {
        self.inner.insert(url, document);
    }

    #[must_use]
    pub fn close(&self, url: &Url) -> Option<TextDocument> {
        self.inner.remove(url).map(|(_, doc)| doc)
    }

    #[must_use]
    pub fn get(&self, url: &Url) -> Option<TextDocument> {
        self.inner.get(url).map(|entry| entry.clone())
    }

    /// Check if a document is open
    #[must_use]
    pub fn contains(&self, url: &Url) -> bool {
        self.inner.contains_key(url)
    }

    pub fn iter(&self) -> impl Iterator<Item = (Url, TextDocument)> + '_ {
        self.inner
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
    }
}

impl Default for Buffers {
    fn default() -> Self {
        Self::new()
    }
}

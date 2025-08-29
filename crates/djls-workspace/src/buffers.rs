//! Shared buffer storage for open documents
//!
//! This module provides the `Buffers` type which represents the in-memory
//! content of open files. These buffers are shared between the Session
//! (which manages document lifecycle) and the WorkspaceFileSystem (which
//! reads from them).

use dashmap::DashMap;
use std::sync::Arc;
use url::Url;

use crate::document::TextDocument;

/// Shared buffer storage between Session and FileSystem
///
/// Buffers represent the in-memory content of open files that takes
/// precedence over disk content when reading through the [`FileSystem`].
/// This is the key abstraction that makes the sharing between Session
/// and [`WorkspaceFileSystem`] explicit and type-safe.
/// 
/// The [`WorkspaceFileSystem`] holds a clone of this structure and checks
/// it before falling back to disk reads.
#[derive(Clone, Debug)]
pub struct Buffers {
    inner: Arc<DashMap<Url, TextDocument>>,
}

impl Buffers {
    /// Create a new empty buffer storage
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(DashMap::new()),
        }
    }

    /// Open a document in the buffers
    pub fn open(&self, url: Url, document: TextDocument) {
        self.inner.insert(url, document);
    }

    /// Update an open document
    pub fn update(&self, url: Url, document: TextDocument) {
        self.inner.insert(url, document);
    }

    /// Close a document and return it if it was open
    #[must_use]
    pub fn close(&self, url: &Url) -> Option<TextDocument> {
        self.inner.remove(url).map(|(_, doc)| doc)
    }

    /// Get a document if it's open
    #[must_use]
    pub fn get(&self, url: &Url) -> Option<TextDocument> {
        self.inner.get(url).map(|entry| entry.clone())
    }

    /// Check if a document is open
    #[must_use]
    pub fn contains(&self, url: &Url) -> bool {
        self.inner.contains_key(url)
    }

    /// Iterate over all open buffers (for debugging)
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


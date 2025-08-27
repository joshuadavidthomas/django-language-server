//! LSP-aware file system wrapper that handles overlays
//!
//! This is the KEY pattern from Ruff - the LspSystem wraps a FileSystem
//! and intercepts reads to check for overlays first. This allows unsaved
//! changes to be used without going through Salsa.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use url::Url;

use crate::system::FileSystem;

/// LSP-aware file system that checks overlays before disk
///
/// This is the critical piece that makes overlays work efficiently in Ruff's
/// architecture. Instead of updating Salsa for every keystroke, we intercept
/// file reads here and return overlay content when available.
pub struct LspSystem {
    /// The underlying file system (usually StdFileSystem)
    inner: Arc<dyn FileSystem>,
    
    /// Map of open document URLs to their overlay content
    overlays: HashMap<Url, String>,
}

impl LspSystem {
    /// Create a new LspSystem wrapping the given file system
    pub fn new(file_system: Arc<dyn FileSystem>) -> Self {
        Self {
            inner: file_system,
            overlays: HashMap::new(),
        }
    }
    
    /// Set overlay content for a document
    pub fn set_overlay(&mut self, url: Url, content: String) {
        self.overlays.insert(url, content);
    }
    
    /// Remove overlay content for a document
    pub fn remove_overlay(&mut self, url: &Url) {
        self.overlays.remove(url);
    }
    
    /// Check if a document has an overlay
    pub fn has_overlay(&self, url: &Url) -> bool {
        self.overlays.contains_key(url)
    }
    
    /// Get overlay content if it exists
    pub fn get_overlay(&self, url: &Url) -> Option<&String> {
        self.overlays.get(url)
    }
    
    /// Convert a URL to a file path
    fn url_to_path(url: &Url) -> Option<PathBuf> {
        if url.scheme() == "file" {
            url.to_file_path().ok().or_else(|| {
                // Fallback for simple conversion
                Some(PathBuf::from(url.path()))
            })
        } else {
            None
        }
    }
}

impl FileSystem for LspSystem {
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        // First check if we have an overlay for this path
        // Convert path to URL for lookup
        let url = Url::from_file_path(path)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Invalid path"))?;
        
        if let Some(content) = self.overlays.get(&url) {
            // Return overlay content instead of reading from disk
            return Ok(content.clone());
        }
        
        // No overlay, read from underlying file system
        self.inner.read_to_string(path)
    }
    
    fn exists(&self, path: &Path) -> bool {
        // Check overlays first
        if let Ok(url) = Url::from_file_path(path) {
            if self.overlays.contains_key(&url) {
                return true;
            }
        }
        
        self.inner.exists(path)
    }
    
    fn is_file(&self, path: &Path) -> bool {
        // Overlays are always files
        if let Ok(url) = Url::from_file_path(path) {
            if self.overlays.contains_key(&url) {
                return true;
            }
        }
        
        self.inner.is_file(path)
    }
    
    fn is_directory(&self, path: &Path) -> bool {
        // Overlays are never directories
        if let Ok(url) = Url::from_file_path(path) {
            if self.overlays.contains_key(&url) {
                return false;
            }
        }
        
        self.inner.is_directory(path)
    }
    
    fn read_directory(&self, path: &Path) -> io::Result<Vec<std::path::PathBuf>> {
        // Overlays don't affect directory listings
        self.inner.read_directory(path)
    }
    
    fn metadata(&self, path: &Path) -> io::Result<std::fs::Metadata> {
        // Can't provide metadata for overlays
        self.inner.metadata(path)
    }
}

/// Extension trait for working with URL-based overlays
pub trait LspSystemExt {
    /// Read file content by URL, checking overlays first
    fn read_url(&self, url: &Url) -> io::Result<String>;
}

impl LspSystemExt for LspSystem {
    fn read_url(&self, url: &Url) -> io::Result<String> {
        // Check overlays first
        if let Some(content) = self.overlays.get(url) {
            return Ok(content.clone());
        }
        
        // Convert URL to path and read from file system
        if let Some(path_buf) = Self::url_to_path(url) {
            self.inner.read_to_string(&path_buf)
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Cannot convert URL to path: {}", url),
            ))
        }
    }
}
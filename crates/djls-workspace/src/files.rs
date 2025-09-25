use std::io;
use std::sync::Arc;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::FxDashMap;
use rustc_hash::FxHashMap;
use url::Url;

use crate::document::TextDocument;
use crate::paths;

pub trait FileSystem: Send + Sync {
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String>;
    fn exists(&self, path: &Utf8Path) -> bool;
}

pub struct InMemoryFileSystem {
    files: FxHashMap<Utf8PathBuf, String>,
}

impl InMemoryFileSystem {
    #[must_use]
    pub fn new() -> Self {
        Self {
            files: FxHashMap::default(),
        }
    }

    pub fn add_file(&mut self, path: Utf8PathBuf, content: String) {
        self.files.insert(path, content);
    }
}

impl Default for InMemoryFileSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl FileSystem for InMemoryFileSystem {
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "File not found"))
    }

    fn exists(&self, path: &Utf8Path) -> bool {
        self.files.contains_key(path)
    }
}

/// Standard file system implementation that uses [`std::fs`].
pub struct OsFileSystem;

impl FileSystem for OsFileSystem {
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn exists(&self, path: &Utf8Path) -> bool {
        path.exists()
    }
}

/// Read-only filesystem overlay that prefers workspace buffers and falls
/// back to disk.
///
/// The overlay makes buffered (in-memory) documents appear as regular files to
/// consumers like Salsa. Any read checks the buffers first and only touches the
/// disk fallback if the file is not open in the workspace.
pub struct OverlayFileSystem {
    /// In-memory buffers that take precedence over disk files
    buffers: Buffers,
    /// Fallback file system for disk operations
    disk: Arc<dyn FileSystem>,
}

impl OverlayFileSystem {
    #[must_use]
    pub fn new(buffers: Buffers, disk: Arc<dyn FileSystem>) -> Self {
        Self { buffers, disk }
    }
}

impl FileSystem for OverlayFileSystem {
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
        if let Some(url) = paths::path_to_url(path) {
            if let Some(document) = self.buffers.get(&url) {
                return Ok(document.content().to_string());
            }
        }
        self.disk.read_to_string(path)
    }

    fn exists(&self, path: &Utf8Path) -> bool {
        paths::path_to_url(path).is_some_and(|url| self.buffers.contains(&url))
            || self.disk.exists(path)
    }
}

/// Shared buffer storage between `Session` and [`FileSystem`].
///
/// Buffers represent the in-memory content of open files that takes
/// precedence over disk content when reading through the [`FileSystem`].
/// This is the key abstraction that makes the sharing between Session
/// and [`OverlayFileSystem`] explicit and type-safe.
///
/// The [`OverlayFileSystem`] holds a clone of this structure and checks
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
/// [`OverlayFileSystem`]: crate::OverlayFileSystem
#[derive(Clone, Debug)]
pub struct Buffers {
    inner: Arc<FxDashMap<Url, TextDocument>>,
}

impl Buffers {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(FxDashMap::default()),
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

#[cfg(test)]
mod tests {
    use super::*;

    mod in_memory {
        use super::*;

        #[test]
        fn test_read_existing_file() {
            let mut fs = InMemoryFileSystem::new();
            fs.add_file("/test.py".into(), "file content".to_string());

            assert_eq!(
                fs.read_to_string(Utf8Path::new("/test.py")).unwrap(),
                "file content"
            );
        }

        #[test]
        fn test_read_nonexistent_file() {
            let fs = InMemoryFileSystem::new();

            let result = fs.read_to_string(Utf8Path::new("/missing.py"));
            assert!(result.is_err());
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotFound);
        }

        #[test]
        fn test_exists_returns_true_for_existing() {
            let mut fs = InMemoryFileSystem::new();
            fs.add_file("/exists.py".into(), "content".to_string());

            assert!(fs.exists(Utf8Path::new("/exists.py")));
        }

        #[test]
        fn test_exists_returns_false_for_nonexistent() {
            let fs = InMemoryFileSystem::new();

            assert!(!fs.exists(Utf8Path::new("/missing.py")));
        }
    }
}

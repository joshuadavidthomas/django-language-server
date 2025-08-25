//! Change-tracked, concurrent virtual file system keyed by [`FileId`].
//!
//! The VFS provides thread-safe, identity-stable storage with cheap change detection
//! and snapshotting. Downstream systems consume snapshots to avoid locking and to
//! batch updates.

use anyhow::{anyhow, Result};
use camino::Utf8PathBuf;
use dashmap::DashMap;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        Arc,
    },
};
use url::Url;

use super::{
    watcher::{VfsWatcher, WatchConfig, WatchEvent},
    FileId,
};

/// Monotonic counter representing global VFS state.
///
/// [`Revision`] increments whenever file content changes occur in the VFS.
/// This provides a cheap way to detect if any changes have occurred since
/// a previous snapshot was taken.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Default, PartialOrd, Ord)]
pub struct Revision(u64);

impl Revision {
    /// Create a [`Revision`] from a raw u64 value.
    #[must_use]
    pub fn from_raw(raw: u64) -> Self {
        Revision(raw)
    }

    /// Get the underlying u64 value.
    #[must_use]
    pub fn value(self) -> u64 {
        self.0
    }
}

/// File classification at the VFS layer.
///
/// [`FileKind`] determines how a file should be processed by downstream analyzers.
/// This classification is performed when files are first ingested into the VFS.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum FileKind {
    /// Python source file
    Python,
    /// Django template file
    Template,
    /// Other file type
    Other,
}

/// Metadata associated with a file in the VFS.
///
/// [`FileMeta`] contains all non-content information about a file, including its
/// identity (URI), filesystem path, and classification.
#[derive(Clone, Debug)]
pub struct FileMeta {
    /// The file's URI (typically file:// scheme)
    pub uri: Url,
    /// The file's path in the filesystem
    pub path: Utf8PathBuf,
    /// Classification for routing to analyzers
    pub kind: FileKind,
}

/// Source of text content in the VFS.
///
/// [`TextSource`] tracks where file content originated from, which is useful for
/// debugging and understanding the current state of the VFS. All variants hold
/// `Arc<str>` for efficient sharing.
#[derive(Clone)]
pub enum TextSource {
    /// Content loaded from disk
    Disk(Arc<str>),
    /// Content from LSP client overlay (in-memory edits)
    Overlay(Arc<str>),
    /// Content generated programmatically
    Generated(Arc<str>),
}

/// Complete record of a file in the VFS.
///
/// [`FileRecord`] combines metadata, current text content, and a content hash
/// for efficient change detection.
#[derive(Clone)]
pub struct FileRecord {
    /// File metadata (URI, path, kind, version)
    pub meta: FileMeta,
    /// Current text content and its source
    pub text: TextSource,
    /// Hash of current content for change detection
    pub hash: u64,
}

/// Thread-safe virtual file system with change tracking.
///
/// [`Vfs`] provides concurrent access to file content with stable [`FileId`] assignment,
/// content hashing for change detection, and atomic snapshot generation. It uses
/// `DashMap` for lock-free concurrent access and atomic counters for revision tracking.
pub struct Vfs {
    /// Atomic counter for generating unique [`FileId`]s
    next_file_id: AtomicU32,
    /// Map from URI to [`FileId`] for deduplication
    by_uri: DashMap<Url, FileId>,
    /// Map from [`FileId`] to [`FileRecord`] for content storage
    files: DashMap<FileId, FileRecord>,
    /// Global revision counter, incremented on content changes
    head: AtomicU64,
    /// Optional file system watcher for external change detection
    watcher: std::sync::Mutex<Option<VfsWatcher>>,
    /// Map from filesystem path to [`FileId`] for watcher events
    by_path: DashMap<Utf8PathBuf, FileId>,
}

impl Vfs {
    /// Get or create a [`FileId`] for the given URI.
    ///
    /// Returns the existing [`FileId`] if the URI is already known, or creates a new
    /// [`FileRecord`] with the provided metadata and text. This method computes and
    /// stores a content hash for change detection.
    pub fn intern_file(
        &self,
        uri: Url,
        path: Utf8PathBuf,
        kind: FileKind,
        text: TextSource,
    ) -> FileId {
        if let Some(id) = self.by_uri.get(&uri).map(|entry| *entry) {
            return id;
        }
        let id = FileId(self.next_file_id.fetch_add(1, Ordering::SeqCst));
        let meta = FileMeta {
            uri: uri.clone(),
            path: path.clone(),
            kind,
        };
        let hash = content_hash(&text);
        self.by_uri.insert(uri, id);
        self.by_path.insert(path, id);
        self.files.insert(id, FileRecord { meta, text, hash });
        id
    }

    /// Set overlay text for a file, typically from LSP didChange events.
    ///
    /// Updates the file's text to an Overlay variant with the new content.
    /// Only increments the global revision if the content actually changed
    /// (detected via hash comparison).
    ///
    /// Returns a tuple of (new global revision, whether content changed).
    pub fn set_overlay(&self, id: FileId, new_text: Arc<str>) -> Result<(Revision, bool)> {
        let mut rec = self
            .files
            .get_mut(&id)
            .ok_or_else(|| anyhow!("unknown file: {:?}", id))?;
        let next = TextSource::Overlay(new_text);
        let new_hash = content_hash(&next);
        let changed = new_hash != rec.hash;
        if changed {
            rec.text = next;
            rec.hash = new_hash;
            self.head.fetch_add(1, Ordering::SeqCst);
        }
        Ok((
            Revision::from_raw(self.head.load(Ordering::SeqCst)),
            changed,
        ))
    }

    /// Create an immutable snapshot of the current VFS state.
    ///
    /// Materializes a consistent view of all files for downstream consumers.
    /// The snapshot includes the current revision and a clone of all file records.
    /// This operation is relatively cheap due to `Arc` sharing of text content.
    pub fn snapshot(&self) -> VfsSnapshot {
        VfsSnapshot {
            revision: Revision::from_raw(self.head.load(Ordering::SeqCst)),
            files: self
                .files
                .iter()
                .map(|entry| (*entry.key(), entry.value().clone()))
                .collect(),
        }
    }

    /// Enable file system watching with the given configuration.
    ///
    /// This starts monitoring the specified root directories for external changes.
    /// Returns an error if file watching is disabled in the config or fails to start.
    pub fn enable_file_watching(&self, config: WatchConfig) -> Result<()> {
        let watcher = VfsWatcher::new(config)?;
        *self
            .watcher
            .lock()
            .map_err(|e| anyhow!("Failed to lock watcher mutex: {}", e))? = Some(watcher);
        Ok(())
    }

    /// Process pending file system events from the watcher.
    ///
    /// This should be called periodically to sync external file changes into the VFS.
    /// Returns the number of files that were updated.
    pub fn process_file_events(&self) -> usize {
        // Get events from the watcher
        let events = {
            let Ok(guard) = self.watcher.lock() else {
                return 0; // Return 0 if mutex is poisoned
            };
            if let Some(watcher) = guard.as_ref() {
                watcher.try_recv_events()
            } else {
                return 0;
            }
        };

        let mut updated_count = 0;

        for event in events {
            match event {
                WatchEvent::Modified(path) | WatchEvent::Created(path) => {
                    if let Err(e) = self.load_from_disk(&path) {
                        eprintln!("Failed to load file from disk: {path}: {e}");
                    } else {
                        updated_count += 1;
                    }
                }
                WatchEvent::Deleted(path) => {
                    // For now, we don't remove deleted files from VFS
                    // This maintains stable `FileId`s for consumers
                    eprintln!("File deleted (keeping in VFS): {path}");
                }
                WatchEvent::Renamed { from, to } => {
                    // Handle rename by updating the path mapping
                    if let Some(file_id) = self.by_path.remove(&from).map(|(_, id)| id) {
                        self.by_path.insert(to.clone(), file_id);
                        if let Err(e) = self.load_from_disk(&to) {
                            eprintln!("Failed to load renamed file: {to}: {e}");
                        } else {
                            updated_count += 1;
                        }
                    }
                }
            }
        }
        updated_count
    }

    /// Load a file's content from disk and update the VFS.
    ///
    /// This method reads the file from the filesystem and updates the VFS entry
    /// if the content has changed. It's used by the file watcher to sync external changes.
    fn load_from_disk(&self, path: &Utf8PathBuf) -> Result<()> {
        // Check if we have this file tracked
        if let Some(file_id) = self.by_path.get(path).map(|entry| *entry.value()) {
            // Read content from disk
            let content = fs::read_to_string(path.as_std_path())
                .map_err(|e| anyhow!("Failed to read file {}: {}", path, e))?;

            let new_text = TextSource::Disk(Arc::from(content.as_str()));
            let new_hash = content_hash(&new_text);

            // Update the file if content changed
            if let Some(mut record) = self.files.get_mut(&file_id) {
                if record.hash != new_hash {
                    record.text = new_text;
                    record.hash = new_hash;
                    self.head.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
        Ok(())
    }

    /// Check if file watching is currently enabled.
    pub fn is_file_watching_enabled(&self) -> bool {
        self.watcher.lock().map(|g| g.is_some()).unwrap_or(false) // Return false if mutex is poisoned
    }
}

impl Default for Vfs {
    fn default() -> Self {
        Self {
            next_file_id: AtomicU32::new(0),
            by_uri: DashMap::new(),
            files: DashMap::new(),
            head: AtomicU64::new(0),
            watcher: std::sync::Mutex::new(None),
            by_path: DashMap::new(),
        }
    }
}

/// Compute a stable hash over file content.
///
/// Used for efficient change detection - if the hash hasn't changed,
/// the content hasn't changed, avoiding unnecessary Salsa invalidations.
fn content_hash(src: &TextSource) -> u64 {
    let s: &str = match src {
        TextSource::Disk(s) | TextSource::Overlay(s) | TextSource::Generated(s) => s,
    };
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Immutable snapshot view of the VFS at a specific revision.
///
/// [`VfsSnapshot`] provides a consistent view of all files for downstream consumers,
/// avoiding the need for locking during processing. Snapshots are created atomically
/// and can be safely shared across threads.
#[derive(Clone)]
pub struct VfsSnapshot {
    /// The global revision at the time of snapshot
    pub revision: Revision,
    /// All files in the VFS at snapshot time
    pub files: HashMap<FileId, FileRecord>,
}

impl VfsSnapshot {
    /// Get the text content of a file in this snapshot.
    ///
    /// Returns `None` if the [`FileId`] is not present in the snapshot.
    #[must_use]
    pub fn get_text(&self, id: FileId) -> Option<Arc<str>> {
        self.files.get(&id).map(|r| match &r.text {
            TextSource::Disk(s) | TextSource::Overlay(s) | TextSource::Generated(s) => s.clone(),
        })
    }

    /// Get the metadata for a file in this snapshot.
    ///
    /// Returns `None` if the [`FileId`] is not present in the snapshot.
    #[must_use]
    pub fn meta(&self, id: FileId) -> Option<&FileMeta> {
        self.files.get(&id).map(|r| &r.meta)
    }
}

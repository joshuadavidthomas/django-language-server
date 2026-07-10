//! Workspace facade for managing open documents and the filesystem overlay.
//!
//! This module owns the mutable LSP-side state: open document buffers and the
//! read-only overlay shared with the Salsa database. The database itself stays
//! pure: it asks its filesystem for source text and receives in-memory contents
//! for open documents before falling back to disk.
//!
//! # Architecture: File-Only URIs (Step 1)
//!
//! The workspace overlay currently only supports `file://` URIs. Documents are
//! keyed by `Utf8PathBuf` for optimal performance in the hot path: overlay reads
//! during source and template parsing.
//!
//! ## Design Decision: Path vs URL Keys
//!
//! DJLS uses path-based keys because Django template features require filesystem
//! context: template loaders, `INSTALLED_APPS`, settings modules, and source
//! roots. Salsa queries are already keyed on paths, and direct path lookups keep
//! the overlay cheap when every file read checks open buffers first.
//!
//! ## Future: Virtual Document Support (Step 2)
//!
//! Virtual documents (`untitled:`, `inmemory:`, etc.) should be supported at
//! this boundary with a document-path enum, not by spreading URI handling through
//! semantic project discovery:
//!
//! ```ignore
//! pub enum DocumentPath {
//!     File(Utf8PathBuf),
//!     Virtual(VirtualPath),
//! }
//! ```
//!
//! This will enable template features to work on unsaved documents while keeping
//! behavior consistent with other LSP servers such as Ruff and Ty.

use std::io;
use std::sync::Arc;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::CaseSensitivity;
use djls_source::FileKind;
use djls_source::FileSystem;
use djls_source::FxDashMap;
use djls_source::OsFileSystem;
use djls_source::PositionEncoding;
use djls_source::WalkEntry;
use djls_source::WalkEntryKind;
use djls_source::WalkOptions;

use crate::document::DocumentChange;
use crate::document::TextDocument;

/// Workspace facade for open buffers and the filesystem overlay.
///
/// `Workspace` stores open documents and exposes an overlay filesystem to the
/// database. `Session` applies the matching project-visible file events after
/// mutating this buffer state.
pub(crate) struct Workspace {
    /// Thread-safe shared buffer storage for open documents.
    buffers: Buffers,
    /// Filesystem abstraction that checks buffers first, then disk.
    overlay: Arc<OverlayFileSystem>,
}

impl Workspace {
    /// Create a workspace with empty buffers and an OS-backed overlay.
    #[must_use]
    pub(crate) fn new() -> Self {
        let buffers = Buffers::new();
        let overlay = Arc::new(OverlayFileSystem::new(
            buffers.clone(),
            Arc::new(OsFileSystem::default()),
        ));

        Self { buffers, overlay }
    }

    /// Return the overlay filesystem for database reads.
    ///
    /// The overlay returns buffer contents when present and falls back to disk
    /// otherwise.
    #[must_use]
    pub(crate) fn overlay(&self) -> Arc<dyn FileSystem> {
        self.overlay.clone()
    }

    /// Return all currently open documents.
    pub(crate) fn open_documents(&self) -> Vec<TextDocument> {
        self.buffers
            .iter()
            .map(|(_path, document)| document)
            .collect()
    }

    #[must_use]
    pub(crate) fn disk_is_file(&self, path: &Utf8Path) -> bool {
        self.overlay.disk.is_file(path)
    }

    #[must_use]
    pub(crate) fn get_document(&self, path: &Utf8Path) -> Option<TextDocument> {
        self.buffers.get(path)
    }

    /// Open a document in memory.
    pub(crate) fn open_document(
        &mut self,
        path: &Utf8Path,
        content: &str,
        version: i32,
        kind: FileKind,
    ) -> TextDocument {
        let document = TextDocument::new(path.to_path_buf(), content.to_string(), version, kind);
        debug_assert_eq!(document.kind(), kind);
        self.buffers.open(path.to_path_buf(), document.clone());
        document
    }

    /// Return the saved open document without changing buffered content.
    pub(crate) fn save_document(&self, path: &Utf8Path) -> Option<TextDocument> {
        self.buffers.get(path)
    }

    /// Apply LSP text changes to an open document.
    pub(crate) fn update_document(
        &mut self,
        path: &Utf8Path,
        changes: Vec<DocumentChange>,
        version: i32,
        encoding: PositionEncoding,
    ) -> Option<TextDocument> {
        if let Some(mut document) = self.buffers.get(path) {
            document.update(changes, version, encoding);
            self.buffers.update(path.to_path_buf(), document.clone());
            Some(document)
        } else if let Some(first_change) = changes.into_iter().next() {
            if first_change.range().is_none() {
                let document = TextDocument::new(
                    path.to_path_buf(),
                    first_change.text().to_string(),
                    version,
                    FileKind::Other,
                );
                self.buffers.open(path.to_path_buf(), document.clone());
                Some(document)
            } else {
                None
            }
        } else {
            None
        }
    }

    /// Close a document, removing it from buffers.
    pub(crate) fn close_document(&mut self, path: &Utf8Path) -> Option<TextDocument> {
        self.buffers.close(path)
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared buffer storage between `Session` and the overlay filesystem.
///
/// Buffers represent the in-memory content of open files that takes precedence
/// over disk content when reading through `OverlayFileSystem`. This is the key
/// abstraction that makes sharing between `Session` and `OverlayFileSystem`
/// explicit and type-safe.
///
/// The `OverlayFileSystem` holds a clone of this structure and checks it before
/// falling back to disk reads.
///
/// ## File URI Requirement (Step 1)
///
/// Currently, this system only supports `file://` URIs. Documents with other URI
/// schemes (for example, `untitled:` or `inmemory:`) are filtered at the LSP
/// boundary.
///
/// Future virtual-document support should extend this type with a document-path
/// enum similar to Ty's `AnySystemPath`, allowing untitled documents to work
/// with limited features.
///
/// ## Memory Management
///
/// This structure does not implement eviction or memory limits because the LSP
/// protocol explicitly manages document lifecycle through `didOpen` and
/// `didClose` notifications. Documents are only stored while the editor has them
/// open, and are properly removed when the editor closes them. This follows the
/// battle-tested pattern used by production LSP servers like Ruff.
#[derive(Clone)]
pub(crate) struct Buffers {
    // TODO(virtual-paths): Change to a document-path key that can represent
    // both real filesystem paths and virtual editor buffers.
    inner: Arc<FxDashMap<Utf8PathBuf, TextDocument>>,
}

impl Buffers {
    #[must_use]
    fn new() -> Self {
        Self {
            inner: Arc::new(FxDashMap::default()),
        }
    }

    fn open(&self, path: Utf8PathBuf, document: TextDocument) {
        self.inner.insert(path, document);
    }

    fn update(&self, path: Utf8PathBuf, document: TextDocument) {
        self.inner.insert(path, document);
    }

    #[must_use]
    fn close(&self, path: &Utf8Path) -> Option<TextDocument> {
        self.inner.remove(path).map(|(_, doc)| doc)
    }

    #[must_use]
    fn get(&self, path: &Utf8Path) -> Option<TextDocument> {
        self.inner.get(path).map(|entry| entry.clone())
    }

    /// Check whether a document is open in memory.
    #[must_use]
    fn contains(&self, path: &Utf8Path) -> bool {
        self.inner.contains_key(path)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (Utf8PathBuf, TextDocument)> + '_ {
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

/// Read-only filesystem overlay that prefers workspace buffers and falls back to disk.
///
/// The overlay makes buffered in-memory documents appear as regular files to
/// consumers like Salsa. Reads, metadata checks, directory listings, and walks
/// check the buffers first and only touch the disk fallback when the file is not
/// open in the workspace.
struct OverlayFileSystem {
    /// In-memory buffers that take precedence over disk files.
    buffers: Buffers,
    /// Fallback filesystem for disk operations.
    disk: Arc<dyn FileSystem>,
}

impl OverlayFileSystem {
    #[must_use]
    fn new(buffers: Buffers, disk: Arc<dyn FileSystem>) -> Self {
        Self { buffers, disk }
    }
}

impl FileSystem for OverlayFileSystem {
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
        // TODO(virtual-paths): Handle DocumentPath::Virtual lookups here.
        // Virtual documents will not have real paths, so they need either a
        // dual-key lookup or a separate virtual document cache.
        if let Some(document) = self.buffers.get(path) {
            return Ok(document.content().to_string());
        }
        self.disk.read_to_string(path)
    }

    fn exists(&self, path: &Utf8Path) -> bool {
        self.buffers.contains(path) || self.is_dir(path) || self.disk.exists(path)
    }

    fn is_file(&self, path: &Utf8Path) -> bool {
        self.buffers.contains(path) || self.disk.is_file(path)
    }

    fn is_dir(&self, path: &Utf8Path) -> bool {
        self.disk.is_dir(path)
            || self.buffers.iter().any(|(buffer_path, _document)| {
                buffer_path.starts_with(path) && buffer_path != path
            })
    }

    fn case_sensitivity(&self) -> CaseSensitivity {
        self.disk.case_sensitivity()
    }

    fn path_exists_case_sensitive(&self, path: &Utf8Path, prefix: &Utf8Path) -> bool {
        self.buffers.contains(path)
            || self.buffers.iter().any(|(buffer_path, _document)| {
                buffer_path.starts_with(path) && buffer_path != path
            })
            || self.disk.path_exists_case_sensitive(path, prefix)
    }

    fn walk_entries(&self, root: &Utf8Path, options: &WalkOptions) -> io::Result<Vec<WalkEntry>> {
        let mut entries = self.disk.walk_entries(root, options)?;

        for (path, _document) in self.buffers.iter() {
            for entry in walk_entries_for_buffer(root, &path, options) {
                if entries.iter().any(|existing| existing.path == entry.path) {
                    continue;
                }
                entries.push(entry);
            }
        }

        entries.sort_by(|left, right| left.path.cmp(&right.path));
        entries.dedup_by(|left, right| left.path == right.path);
        Ok(entries)
    }
}

fn walk_entries_for_buffer(
    root: &Utf8Path,
    path: &Utf8Path,
    options: &WalkOptions,
) -> Vec<WalkEntry> {
    let Some(relative) = buffer_relative_path(root, path) else {
        return Vec::new();
    };
    if path == root {
        return vec![WalkEntry::file_root(root)];
    }

    let mut entries = Vec::new();
    let mut entry_path = root.to_path_buf();
    let mut entry_relative = Utf8PathBuf::new();
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        entry_path.push(component.as_str());
        entry_relative.push(component.as_str());

        if !options.hidden
            && entry_relative
                .components()
                .any(|component| component.as_str().starts_with('.') && component.as_str() != ".")
        {
            continue;
        }
        if let Some(max_depth) = options.max_depth
            && entry_relative.components().count() > max_depth
        {
            continue;
        }

        entries.push(WalkEntry {
            root: root.to_path_buf(),
            path: entry_path.clone(),
            relative: entry_relative.clone(),
            kind: if components.peek().is_some() {
                WalkEntryKind::Directory
            } else {
                WalkEntryKind::File
            },
        });
    }

    entries
}

fn buffer_relative_path(root: &Utf8Path, path: &Utf8Path) -> Option<Utf8PathBuf> {
    if path == root {
        return path.file_name().map(Utf8PathBuf::from);
    }

    path.strip_prefix(root).ok().map(Utf8Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use djls_source::ChangeEvent;
    use djls_source::Db as _;
    use djls_source::FileRootKind;
    use djls_source::InMemoryFileSystem;
    use djls_source::SourceChanges;
    use djls_source::SourceFiles;
    use djls_source::path_to_file;
    use tempfile::tempdir;

    use super::*;

    #[salsa::db]
    #[derive(Clone)]
    struct TestDb {
        storage: salsa::Storage<Self>,
        fs: Arc<dyn FileSystem>,
        files: SourceFiles,
    }

    impl TestDb {
        fn new(fs: Arc<dyn FileSystem>) -> Self {
            Self {
                storage: salsa::Storage::default(),
                fs,
                files: SourceFiles::default(),
            }
        }
    }

    #[salsa::db]
    impl salsa::Database for TestDb {}

    #[salsa::db]
    impl djls_source::Db for TestDb {
        fn files(&self) -> &SourceFiles {
            &self.files
        }

        fn file_system(&self) -> &dyn FileSystem {
            self.fs.as_ref()
        }
    }

    #[salsa::db]
    impl djls_project::Db for TestDb {
        fn project(&self) -> Option<djls_project::Project> {
            None
        }
    }

    fn text_document(_db: &TestDb, path: &Utf8Path, content: &str) -> TextDocument {
        TextDocument::new(path.to_path_buf(), content.to_string(), 1, FileKind::Python)
    }

    #[test]
    fn overlay_reads_from_buffer_before_disk() {
        let mut disk = InMemoryFileSystem::new();
        let path = Utf8PathBuf::from("/project/app.py");
        disk.add_file(path.clone(), "disk content".to_string());

        let buffers = Buffers::new();
        let fs = OverlayFileSystem::new(buffers.clone(), Arc::new(disk));
        let db = TestDb::new(Arc::new(InMemoryFileSystem::new()));
        buffers.open(path.clone(), text_document(&db, &path, "buffer content"));

        assert_eq!(fs.read_to_string(&path).unwrap(), "buffer content");
    }

    #[test]
    fn overlay_walk_includes_buffer_only_file() {
        let buffers = Buffers::new();
        let fs = OverlayFileSystem::new(buffers.clone(), Arc::new(InMemoryFileSystem::new()));
        let db = TestDb::new(Arc::new(InMemoryFileSystem::new()));
        let root = Utf8Path::new("/project/templates");
        let path = Utf8PathBuf::from("/project/templates/buffer.html");
        buffers.open(path.clone(), text_document(&db, &path, "buffer"));

        let entries = fs.walk_entries(root, &WalkOptions::unrestricted()).unwrap();
        let relatives: Vec<_> = entries
            .iter()
            .map(|entry| entry.relative.as_str())
            .collect();

        assert_eq!(relatives, vec!["buffer.html"]);
    }

    #[test]
    fn overlay_case_sensitive_exists_includes_buffer_implied_directory() {
        let buffers = Buffers::new();
        let fs = OverlayFileSystem::new(buffers.clone(), Arc::new(InMemoryFileSystem::new()));
        let db = TestDb::new(Arc::new(InMemoryFileSystem::new()));
        let path = Utf8PathBuf::from("/project/pkg/bar.py");
        buffers.open(path.clone(), text_document(&db, &path, "buffer"));

        assert!(fs.path_exists_case_sensitive(
            Utf8Path::new("/project/pkg"),
            Utf8Path::new("/project"),
        ));
    }

    #[test]
    fn overlay_walk_respects_hidden_option_for_buffers() {
        let buffers = Buffers::new();
        let fs = OverlayFileSystem::new(buffers.clone(), Arc::new(InMemoryFileSystem::new()));
        let db = TestDb::new(Arc::new(InMemoryFileSystem::new()));
        let root = Utf8Path::new("/project");
        let hidden_path = Utf8PathBuf::from("/project/.hidden/secret.html");
        let visible_path = Utf8PathBuf::from("/project/visible.html");
        buffers.open(
            hidden_path.clone(),
            text_document(&db, &hidden_path, "secret"),
        );
        buffers.open(
            visible_path.clone(),
            text_document(&db, &visible_path, "visible"),
        );

        let entries = fs.walk_entries(root, &WalkOptions::default()).unwrap();
        let relatives: Vec<_> = entries
            .iter()
            .map(|entry| entry.relative.as_str())
            .collect();

        assert_eq!(relatives, vec!["visible.html"]);
    }

    #[test]
    fn disk_backed_open_and_close_do_not_bump_root_revision() {
        let temp_dir = tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(temp_dir.path().to_path_buf()).unwrap();
        let file_path = root.join("template.html");
        std::fs::write(file_path.as_std_path(), "disk template").unwrap();

        let mut workspace = Workspace::new();
        let mut db = TestDb::new(workspace.overlay());
        let root = db
            .files()
            .try_add_root(&db, root.to_path_buf(), FileRootKind::Project);
        let _ = path_to_file(&db, &file_path).expect("disk-backed fixture should exist");

        workspace.open_document(&file_path, "buffer template", 1, FileKind::Template);
        SourceChanges::new([ChangeEvent::Opened(file_path.clone())]).apply(&mut db);
        assert_eq!(root.revision(&db), 0);

        workspace.close_document(&file_path).unwrap();
        SourceChanges::new([ChangeEvent::ContentChanged(file_path.clone())]).apply(&mut db);
        assert_eq!(root.revision(&db), 0);
    }

    #[test]
    fn untracked_disk_backed_open_bumps_root_revision() {
        let temp_dir = tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(temp_dir.path().to_path_buf()).unwrap();
        let file_path = root.join("template.html");
        std::fs::write(file_path.as_std_path(), "disk template").unwrap();

        let mut workspace = Workspace::new();
        let mut db = TestDb::new(workspace.overlay());
        let root = db
            .files()
            .try_add_root(&db, root.to_path_buf(), FileRootKind::Project);

        workspace.open_document(&file_path, "buffer template", 1, FileKind::Template);
        SourceChanges::new([ChangeEvent::BecameVisible(file_path.clone())]).apply(&mut db);
        assert_eq!(root.revision(&db), 1);

        workspace.close_document(&file_path).unwrap();
        SourceChanges::new([ChangeEvent::ContentChanged(file_path.clone())]).apply(&mut db);
        assert_eq!(root.revision(&db), 1);
    }

    #[test]
    fn buffer_only_open_and_close_bump_root_revision() {
        let temp_dir = tempdir().unwrap();
        let root_path = Utf8PathBuf::from_path_buf(temp_dir.path().to_path_buf()).unwrap();
        let file_path = root_path.join("template.html");

        let mut workspace = Workspace::new();
        let mut db = TestDb::new(workspace.overlay());
        let root = db
            .files()
            .try_add_root(&db, root_path.to_path_buf(), FileRootKind::Project);

        workspace.open_document(&file_path, "buffer template", 1, FileKind::Template);
        SourceChanges::new([ChangeEvent::BecameVisible(file_path.clone())]).apply(&mut db);
        assert_eq!(root.revision(&db), 1);

        workspace.close_document(&file_path).unwrap();
        SourceChanges::new([ChangeEvent::Deleted(file_path.clone())]).apply(&mut db);
        assert_eq!(root.revision(&db), 2);
    }

    #[test]
    fn workspace_open_update_and_close_flow_through_source_files() {
        let temp_dir = tempdir().unwrap();
        let file_path = Utf8PathBuf::from_path_buf(temp_dir.path().join("template.html")).unwrap();
        std::fs::write(file_path.as_std_path(), "disk template").unwrap();

        let mut workspace = Workspace::new();
        let mut db = TestDb::new(workspace.overlay());
        workspace.open_document(&file_path, "buffer template", 1, FileKind::Template);
        SourceChanges::new([ChangeEvent::BecameVisible(file_path.clone())]).apply(&mut db);
        let file = path_to_file(&db, &file_path).expect("opened document should be interned");
        assert_eq!(
            file.try_source(&db)
                .expect("buffer should be readable")
                .as_str(),
            "buffer template"
        );

        std::fs::write(file_path.as_std_path(), "changed disk template").unwrap();
        SourceChanges::new([ChangeEvent::Rescan]).apply(&mut db);
        assert_eq!(
            file.try_source(&db)
                .expect("overlay should remain authoritative during rescan")
                .as_str(),
            "buffer template"
        );

        workspace
            .update_document(
                &file_path,
                vec![DocumentChange::new(None, "updated template".to_string())],
                2,
                PositionEncoding::Utf16,
            )
            .unwrap();
        SourceChanges::new([ChangeEvent::ContentChanged(file_path.clone())]).apply(&mut db);
        assert_eq!(
            file.try_source(&db)
                .expect("buffer should be readable")
                .as_str(),
            "updated template"
        );

        workspace.close_document(&file_path).unwrap();
        SourceChanges::new([ChangeEvent::ContentChanged(file_path.clone())]).apply(&mut db);
        assert_eq!(
            file.try_source(&db)
                .expect("disk file should be readable")
                .as_str(),
            "changed disk template"
        );
    }
}

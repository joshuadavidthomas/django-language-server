//! Bridge between VFS snapshots and Salsa inputs.
//!
//! The bridge module isolates Salsa input mutation behind a single, idempotent API.
//! It ensures we only touch Salsa when content or classification changes, maximizing
//! incremental performance.

use std::{collections::HashMap, sync::Arc};

use salsa::Setter;

use super::{
    db::{Database, FileKindMini, SourceFile, TemplateLoaderOrder},
    vfs::{FileKind, VfsSnapshot},
    FileId,
};

/// Owner of the Salsa [`Database`] plus the handles for updating inputs.
///
/// [`FileStore`] serves as the bridge between the VFS (with [`FileId`]s) and Salsa (with entities).
/// It maintains a mapping from [`FileId`]s to [`SourceFile`] entities and manages the global
/// [`TemplateLoaderOrder`] input. The [`FileStore`] ensures that Salsa inputs are only mutated
/// when actual changes occur, preserving incremental computation efficiency.
pub struct FileStore {
    /// The Salsa DB instance
    pub db: Database,
    /// Map from [`FileId`] to its Salsa input entity
    files: HashMap<FileId, SourceFile>,
    /// Handle to the global template loader configuration input
    template_loader: Option<TemplateLoaderOrder>,
}

impl FileStore {
    /// Construct an empty store and DB.
    pub fn new() -> Self {
        Self {
            db: Database::default(),
            files: HashMap::new(),
            template_loader: None,
        }
    }

    /// Create or update the global template loader order input.
    ///
    /// Sets the ordered list of template root directories that Django will search
    /// when resolving template names. If the input already exists, it updates the
    /// existing value; otherwise, it creates a new [`TemplateLoaderOrder`] input.
    pub fn set_template_loader_order(&mut self, ordered_roots: Vec<String>) {
        let roots = Arc::from(ordered_roots.into_boxed_slice());
        if let Some(tl) = self.template_loader {
            tl.set_roots(&mut self.db).to(roots);
        } else {
            self.template_loader = Some(TemplateLoaderOrder::new(&self.db, roots));
        }
    }

    /// Mirror a VFS snapshot into Salsa inputs.
    ///
    /// This method is the core synchronization point between the VFS and Salsa.
    /// It iterates through all files in the snapshot and:
    /// - Creates [`SourceFile`] inputs for new files
    /// - Updates `.text` and `.kind` only when changed to preserve incremental reuse
    ///
    /// The method is idempotent and minimizes Salsa invalidations by checking for
    /// actual changes before updating inputs.
    pub fn apply_vfs_snapshot(&mut self, snap: &VfsSnapshot) {
        for (id, rec) in &snap.files {
            let new_text = snap.get_text(*id).unwrap_or_else(|| Arc::<str>::from(""));
            let new_kind = match rec.meta.kind {
                FileKind::Python => FileKindMini::Python,
                FileKind::Template => FileKindMini::Template,
                FileKind::Other => FileKindMini::Other,
            };

            if let Some(sf) = self.files.get(id) {
                // Update if changed — avoid touching Salsa when not needed
                if sf.kind(&self.db) != new_kind {
                    sf.set_kind(&mut self.db).to(new_kind.clone());
                }
                if sf.text(&self.db).as_ref() != &*new_text {
                    sf.set_text(&mut self.db).to(new_text.clone());
                }
            } else {
                let sf = SourceFile::new(&self.db, new_kind, new_text);
                self.files.insert(*id, sf);
            }
        }
    }

    /// Get the text content of a file by its [`FileId`].
    ///
    /// Returns `None` if the file is not tracked in the [`FileStore`].
    pub fn file_text(&self, id: FileId) -> Option<Arc<str>> {
        self.files.get(&id).map(|sf| sf.text(&self.db).clone())
    }

    /// Get the file kind classification by its [`FileId`].
    ///
    /// Returns `None` if the file is not tracked in the [`FileStore`].
    pub fn file_kind(&self, id: FileId) -> Option<FileKindMini> {
        self.files.get(&id).map(|sf| sf.kind(&self.db))
    }
}

impl Default for FileStore {
    fn default() -> Self {
        Self::new()
    }
}

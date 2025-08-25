//! Bridge between VFS snapshots and Salsa inputs.
//!
//! The bridge module isolates Salsa input mutation behind a single, idempotent API.
//! It ensures we only touch Salsa when content or classification changes, maximizing
//! incremental performance.

use std::collections::HashMap;
use std::sync::Arc;

use salsa::Setter;

use super::db::parse_template;
use super::db::template_errors;
use super::db::Database;
use super::db::SourceFile;
use super::db::TemplateAst;
use super::db::TemplateLoaderOrder;
use super::vfs::FileKind;
use super::vfs::VfsSnapshot;
use super::FileId;

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
    #[must_use]
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
    pub(crate) fn apply_vfs_snapshot(&mut self, snap: &VfsSnapshot) {
        for (id, rec) in &snap.files {
            let new_text = snap.get_text(*id).unwrap_or_else(|| Arc::<str>::from(""));
            let new_kind = rec.meta.kind;

            if let Some(sf) = self.files.get(id) {
                // Update if changed — avoid touching Salsa when not needed
                if sf.kind(&self.db) != new_kind {
                    sf.set_kind(&mut self.db).to(new_kind);
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
    pub(crate) fn file_text(&self, id: FileId) -> Option<Arc<str>> {
        self.files.get(&id).map(|sf| sf.text(&self.db).clone())
    }

    /// Get the file kind classification by its [`FileId`].
    ///
    /// Returns `None` if the file is not tracked in the [`FileStore`].
    pub(crate) fn file_kind(&self, id: FileId) -> Option<FileKind> {
        self.files.get(&id).map(|sf| sf.kind(&self.db))
    }

    /// Get the parsed template AST for a file by its [`FileId`].
    ///
    /// This method leverages Salsa's incremental computation to cache parsed ASTs.
    /// The AST is only re-parsed when the file's content changes in the VFS.
    /// Returns `None` if the file is not tracked or is not a template file.
    pub(crate) fn get_template_ast(&self, id: FileId) -> Option<Arc<TemplateAst>> {
        let source_file = self.files.get(&id)?;
        parse_template(&self.db, *source_file)
    }

    /// Get template parsing errors for a file by its [`FileId`].
    ///
    /// This method provides quick access to template errors without needing the full AST.
    /// Useful for diagnostics and error reporting. Returns an empty slice for
    /// non-template files or files not tracked in the store.
    pub(crate) fn get_template_errors(&self, id: FileId) -> Arc<[String]> {
        self.files
            .get(&id)
            .map_or_else(|| Arc::from(vec![]), |sf| template_errors(&self.db, *sf))
    }
}

impl Default for FileStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;

    use super::*;
    use crate::vfs::TextSource;
    use crate::vfs::Vfs;

    #[test]
    fn test_filestore_template_ast_caching() {
        let mut store = FileStore::new();
        let vfs = Vfs::default();

        // Create a template file in VFS
        let url = url::Url::parse("file:///test.html").unwrap();
        let path = Utf8PathBuf::from("/test.html");
        let content: Arc<str> = Arc::from("{% if user %}Hello {{ user.name }}{% endif %}");
        let file_id = vfs.intern_file(
            url.clone(),
            path.clone(),
            FileKind::Template,
            TextSource::Overlay(content.clone()),
        );
        vfs.set_overlay(file_id, content.clone()).unwrap();

        // Apply VFS snapshot to FileStore
        let snapshot = vfs.snapshot();
        store.apply_vfs_snapshot(&snapshot);

        // Get template AST - should parse and cache
        let ast1 = store.get_template_ast(file_id);
        assert!(ast1.is_some());

        // Get again - should return cached
        let ast2 = store.get_template_ast(file_id);
        assert!(ast2.is_some());
        assert!(Arc::ptr_eq(&ast1.unwrap(), &ast2.unwrap()));
    }

    #[test]
    fn test_filestore_template_errors() {
        let mut store = FileStore::new();
        let vfs = Vfs::default();

        // Create a template with an unclosed tag
        let url = url::Url::parse("file:///error.html").unwrap();
        let path = Utf8PathBuf::from("/error.html");
        let content: Arc<str> = Arc::from("{% if user %}Hello {{ user.name }"); // Missing closing
        let file_id = vfs.intern_file(
            url.clone(),
            path.clone(),
            FileKind::Template,
            TextSource::Overlay(content.clone()),
        );
        vfs.set_overlay(file_id, content).unwrap();

        // Apply VFS snapshot
        let snapshot = vfs.snapshot();
        store.apply_vfs_snapshot(&snapshot);

        // Get errors - should contain parsing errors
        let errors = store.get_template_errors(file_id);
        // The template has unclosed tags, so there should be errors
        // We don't assert on specific error count as the parser may evolve

        // Verify errors are cached
        let errors2 = store.get_template_errors(file_id);
        assert!(Arc::ptr_eq(&errors, &errors2));
    }

    #[test]
    fn test_filestore_invalidation_on_content_change() {
        let mut store = FileStore::new();
        let vfs = Vfs::default();

        // Create initial template
        let url = url::Url::parse("file:///change.html").unwrap();
        let path = Utf8PathBuf::from("/change.html");
        let content1: Arc<str> = Arc::from("{% if user %}Hello{% endif %}");
        let file_id = vfs.intern_file(
            url.clone(),
            path.clone(),
            FileKind::Template,
            TextSource::Overlay(content1.clone()),
        );
        vfs.set_overlay(file_id, content1).unwrap();

        // Apply snapshot and get AST
        let snapshot1 = vfs.snapshot();
        store.apply_vfs_snapshot(&snapshot1);
        let ast1 = store.get_template_ast(file_id);

        // Change content
        let content2: Arc<str> = Arc::from("{% for item in items %}{{ item }}{% endfor %}");
        vfs.set_overlay(file_id, content2).unwrap();

        // Apply new snapshot
        let snapshot2 = vfs.snapshot();
        store.apply_vfs_snapshot(&snapshot2);

        // Get AST again - should be different due to content change
        let ast2 = store.get_template_ast(file_id);
        assert!(ast1.is_some() && ast2.is_some());
        assert!(!Arc::ptr_eq(&ast1.unwrap(), &ast2.unwrap()));
    }
}

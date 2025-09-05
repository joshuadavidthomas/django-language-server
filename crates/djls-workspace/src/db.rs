//! Salsa database for incremental computation.
//!
//! This module provides the [`Database`] which integrates with Salsa for
//! incremental computation of Django template parsing and analysis.
//!
//! ## Architecture
//!
//! The system uses a two-layer approach:
//! 1. **Buffer layer** ([`Buffers`]) - Stores open document content in memory
//! 2. **Salsa layer** ([`Database`]) - Tracks files and computes derived queries
//!
//! When Salsa needs file content, it calls [`source_text`] which:
//! 1. Creates a dependency on the file's revision (critical!)
//! 2. Reads through [`WorkspaceFileSystem`] which checks buffers first
//! 3. Falls back to disk if no buffer exists
//!
//! ## The Revision Dependency
//!
//! The [`source_text`] function **must** call `file.revision(db)` to create
//! a Salsa dependency. Without this, revision changes won't invalidate queries:
//!
//! ```ignore
//! let _ = file.revision(db);  // Creates the dependency chain!
//! ```
//!
//! [`Buffers`]: crate::buffers::Buffers
//! [`WorkspaceFileSystem`]: crate::fs::WorkspaceFileSystem

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;

use dashmap::DashMap;
use salsa::Setter;

use crate::FileKind;
use crate::FileSystem;

/// Database trait that provides file system access for Salsa queries
#[salsa::db]
pub trait Db: salsa::Database {
    /// Get the file system for reading files.
    fn fs(&self) -> Arc<dyn FileSystem>;

    /// Read file content through the file system.
    ///
    /// Checks buffers first via [`WorkspaceFileSystem`](crate::fs::WorkspaceFileSystem),
    /// then falls back to disk.
    fn read_file_content(&self, path: &Path) -> std::io::Result<String>;
}

/// Salsa database for incremental computation.
///
/// Tracks files and computes derived queries incrementally. Integrates with
/// [`WorkspaceFileSystem`](crate::fs::WorkspaceFileSystem) to read file content,
/// which checks buffers before falling back to disk.
#[salsa::db]
#[derive(Clone)]
pub struct Database {
    storage: salsa::Storage<Self>,

    /// File system for reading file content (checks buffers first, then disk).
    fs: Arc<dyn FileSystem>,

    /// Maps paths to [`SourceFile`] entities for O(1) lookup.
    files: Arc<DashMap<PathBuf, SourceFile>>,

    // The logs are only used for testing and demonstrating reuse:
    #[cfg(test)]
    #[allow(dead_code)]
    logs: Arc<Mutex<Option<Vec<String>>>>,
}

#[cfg(test)]
impl Default for Database {
    fn default() -> Self {
        use crate::fs::InMemoryFileSystem;

        let logs = <Arc<Mutex<Option<Vec<String>>>>>::default();
        Self {
            storage: salsa::Storage::new(Some(Box::new({
                let logs = logs.clone();
                move |event| {
                    eprintln!("Event: {event:?}");
                    // Log interesting events, if logging is enabled
                    if let Some(logs) = &mut *logs.lock().unwrap() {
                        // only log interesting events
                        if let salsa::EventKind::WillExecute { .. } = event.kind {
                            logs.push(format!("Event: {event:?}"));
                        }
                    }
                }
            }))),
            fs: Arc::new(InMemoryFileSystem::new()),
            files: Arc::new(DashMap::new()),
            logs,
        }
    }
}

impl Database {
    pub fn new(file_system: Arc<dyn FileSystem>, files: Arc<DashMap<PathBuf, SourceFile>>) -> Self {
        Self {
            storage: salsa::Storage::new(None),
            fs: file_system,
            files,
            #[cfg(test)]
            logs: Arc::new(Mutex::new(None)),
        }
    }

    /// Read file content through the file system.
    pub fn read_file_content(&self, path: &Path) -> std::io::Result<String> {
        self.fs.read_to_string(path)
    }

    /// Get an existing [`SourceFile`] for the given path without creating it.
    ///
    /// Returns `Some(SourceFile)` if the file is already tracked, `None` otherwise.
    /// This method uses an immutable reference and doesn't modify the database.
    pub fn get_file(&self, path: &Path) -> Option<SourceFile> {
        self.files.get(path).map(|file_ref| *file_ref)
    }

    /// Get or create a [`SourceFile`] for the given path.
    ///
    /// Files are created with an initial revision of 0 and tracked in the [`Database`]'s
    /// `DashMap`. The `Arc` ensures cheap cloning while maintaining thread safety.
    ///
    /// ## Thread Safety
    ///
    /// This method is inherently thread-safe despite the check-then-create pattern because
    /// it requires `&mut self`, ensuring exclusive access to the Database. Only one thread
    /// can call this method at a time due to Rust's ownership rules.
    pub fn get_or_create_file(&mut self, path: &PathBuf) -> SourceFile {
        if let Some(file_ref) = self.files.get(path) {
            // Copy the value (SourceFile is Copy)
            // The guard drops automatically, no need for explicit drop
            return *file_ref;
        }

        // File doesn't exist, so we need to create it
        let kind = FileKind::from_path(path);
        let file = SourceFile::new(self, kind, Arc::from(path.to_string_lossy().as_ref()), 0);

        self.files.insert(path.clone(), file);
        file
    }

    /// Check if a file is being tracked without creating it.
    ///
    /// This is primarily used for testing to verify that files have been
    /// created without affecting the database state.
    pub fn has_file(&self, path: &Path) -> bool {
        self.files.contains_key(path)
    }

    /// Touch a file to mark it as modified, triggering re-evaluation of dependent queries.
    ///
    /// Similar to Unix `touch`, this updates the file's revision number to signal
    /// that cached query results depending on this file should be invalidated.
    ///
    /// This is typically called when:
    /// - A file is opened in the editor (if it was previously cached from disk)
    /// - A file's content is modified
    /// - A file's buffer is closed (reverting to disk content)
    pub fn touch_file(&mut self, path: &Path) {
        // Get the file if it exists
        let Some(file_ref) = self.files.get(path) else {
            tracing::debug!("File {} not tracked, skipping touch", path.display());
            return;
        };
        let file = *file_ref;
        drop(file_ref); // Explicitly drop to release the lock

        let current_rev = file.revision(self);
        let new_rev = current_rev + 1;
        file.set_revision(self).to(new_rev);

        tracing::debug!(
            "Touched {}: revision {} -> {}",
            path.display(),
            current_rev,
            new_rev
        );
    }
}

#[salsa::db]
impl salsa::Database for Database {}

#[salsa::db]
impl Db for Database {
    fn fs(&self) -> Arc<dyn FileSystem> {
        self.fs.clone()
    }

    fn read_file_content(&self, path: &Path) -> std::io::Result<String> {
        self.fs.read_to_string(path)
    }
}

/// Represents a single file without storing its content.
///
/// [`SourceFile`] is a Salsa input entity that tracks a file's path, revision, and
/// classification for analysis routing. Following Ruff's pattern, content is NOT
/// stored here but read on-demand through the `source_text` tracked function.
#[salsa::input]
pub struct SourceFile {
    /// The file's classification for analysis routing
    pub kind: FileKind,
    /// The file path
    #[returns(ref)]
    pub path: Arc<str>,
    /// The revision number for invalidation tracking
    pub revision: u64,
}

/// Read file content, creating a Salsa dependency on the file's revision.
#[salsa::tracked]
pub fn source_text(db: &dyn Db, file: SourceFile) -> Arc<str> {
    // This line creates the Salsa dependency on revision! Without this call,
    // revision changes won't trigger invalidation
    let _ = file.revision(db);

    let path = Path::new(file.path(db).as_ref());
    match db.read_file_content(path) {
        Ok(content) => Arc::from(content),
        Err(_) => {
            Arc::from("") // Return empty string for missing files
        }
    }
}

/// Represents a file path for Salsa tracking.
///
/// [`FilePath`] is a Salsa input entity that tracks a file path for use in
/// path-based queries. This allows Salsa to properly track dependencies
/// on files identified by path rather than by SourceFile input.
#[salsa::input]
pub struct FilePath {
    /// The file path as a string
    #[returns(ref)]
    pub path: Arc<str>,
}

/// Container for a parsed Django template AST.
///
/// [`TemplateAst`] wraps the parsed AST from djls-templates along with any parsing errors.
/// This struct is designed to be cached by Salsa and shared across multiple consumers
/// without re-parsing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateAst {
    /// The parsed AST from djls-templates
    pub ast: djls_templates::Ast,
    /// Any errors encountered during parsing (stored as strings for simplicity)
    pub errors: Vec<String>,
}

/// Parse a Django template file into an AST.
///
/// This Salsa tracked function parses template files on-demand and caches the results.
/// The parse is only re-executed when the file's content changes (detected via content changes).
///
/// Returns `None` for non-template files.
#[salsa::tracked]
pub fn parse_template(db: &dyn Db, file: SourceFile) -> Option<Arc<TemplateAst>> {
    // Only parse template files
    if file.kind(db) != FileKind::Template {
        return None;
    }

    let text_arc = source_text(db, file);
    let text = text_arc.as_ref();

    // Call the pure parsing function from djls-templates
    // TODO: Move this whole function into djls-templates
    match djls_templates::parse_template(text) {
        Ok((ast, errors)) => {
            // Convert errors to strings
            let error_strings = errors.into_iter().map(|e| e.to_string()).collect();
            Some(Arc::new(TemplateAst {
                ast,
                errors: error_strings,
            }))
        }
        Err(err) => {
            // Even on fatal errors, return an empty AST with the error
            Some(Arc::new(TemplateAst {
                ast: djls_templates::Ast::default(),
                errors: vec![err.to_string()],
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use dashmap::DashMap;
    use salsa::Setter;

    use super::*;
    use crate::buffers::Buffers;
    use crate::document::TextDocument;
    use crate::fs::InMemoryFileSystem;
    use crate::fs::WorkspaceFileSystem;
    use crate::language::LanguageId;

    #[test]
    fn test_parse_template_with_overlay() {
        // Create a memory filesystem with initial template content
        let mut memory_fs = InMemoryFileSystem::new();
        let template_path = PathBuf::from("/test/template.html");
        memory_fs.add_file(
            template_path.clone(),
            "{% block content %}Original{% endblock %}".to_string(),
        );

        // Create overlay storage
        let buffers = Buffers::new();

        // Create WorkspaceFileSystem that checks overlays first
        let file_system = Arc::new(WorkspaceFileSystem::new(
            buffers.clone(),
            Arc::new(memory_fs),
        ));

        // Create database with the file system
        let files = Arc::new(DashMap::new());
        let mut db = Database::new(file_system, files);

        // Create a SourceFile for the template
        let file = db.get_or_create_file(&template_path);

        // Parse template - should get original content from disk
        let ast1 = parse_template(&db, file).expect("Should parse template");
        assert!(ast1.errors.is_empty(), "Should have no errors");

        // Add an overlay with updated content
        let url = crate::paths::path_to_url(&template_path).unwrap();
        let updated_document = TextDocument::new(
            "{% block content %}Updated from overlay{% endblock %}".to_string(),
            2,
            LanguageId::Other,
        );
        buffers.open(url, updated_document);

        // Bump the file revision to trigger re-parse
        file.set_revision(&mut db).to(1);

        // Parse again - should now get overlay content
        let ast2 = parse_template(&db, file).expect("Should parse template");
        assert!(ast2.errors.is_empty(), "Should have no errors");

        // Verify the content changed (we can't directly check the text,
        // but the AST should be different)
        // The AST will have different content in the block
        assert_ne!(
            format!("{:?}", ast1.ast),
            format!("{:?}", ast2.ast),
            "AST should change when overlay is added"
        );
    }

    #[test]
    fn test_parse_template_invalidation_on_revision_change() {
        // Create a memory filesystem
        let mut memory_fs = InMemoryFileSystem::new();
        let template_path = PathBuf::from("/test/template.html");
        memory_fs.add_file(
            template_path.clone(),
            "{% if true %}Initial{% endif %}".to_string(),
        );

        // Create overlay storage
        let buffers = Buffers::new();

        // Create WorkspaceFileSystem
        let file_system = Arc::new(WorkspaceFileSystem::new(
            buffers.clone(),
            Arc::new(memory_fs),
        ));

        // Create database
        let files = Arc::new(DashMap::new());
        let mut db = Database::new(file_system, files);

        // Create a SourceFile for the template
        let file = db.get_or_create_file(&template_path);

        // Parse template first time
        let ast1 = parse_template(&db, file).expect("Should parse");

        // Parse again without changing revision - should return same Arc (cached)
        let ast2 = parse_template(&db, file).expect("Should parse");
        assert!(Arc::ptr_eq(&ast1, &ast2), "Should return cached result");

        // Update overlay content
        let url = crate::paths::path_to_url(&template_path).unwrap();
        let updated_document = TextDocument::new(
            "{% if false %}Changed{% endif %}".to_string(),
            2,
            LanguageId::Other,
        );
        buffers.open(url, updated_document);

        // Bump revision to trigger invalidation
        file.set_revision(&mut db).to(1);

        // Parse again - should get different result due to invalidation
        let ast3 = parse_template(&db, file).expect("Should parse");
        assert!(
            !Arc::ptr_eq(&ast1, &ast3),
            "Should re-execute after revision change"
        );

        // Content should be different
        assert_ne!(
            format!("{:?}", ast1.ast),
            format!("{:?}", ast3.ast),
            "AST should be different after content change"
        );
    }
}

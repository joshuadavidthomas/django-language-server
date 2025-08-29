//! Salsa database and input entities for workspace.
//!
//! This module implements a two-layer architecture inspired by Ruff's design pattern
//! for efficient LSP document management with Salsa incremental computation.
//!
//! # Two-Layer Architecture
//!
//! ## Layer 1: LSP Document Management (in Session)
//! - Stores overlays in `Session` using `Arc<DashMap<Url, TextDocument>>`
//! - TextDocument contains actual content, version, language_id
//! - Changes are immediate, no Salsa invalidation on every keystroke
//! - Thread-safe via DashMap for tower-lsp's Send+Sync requirements
//!
//! ## Layer 2: Salsa Incremental Computation (in Database)
//! - Database is pure Salsa, no file content storage
//! - Files tracked via `Arc<DashMap<PathBuf, SourceFile>>` for O(1) lookups
//! - SourceFile inputs only have path and revision (no text)
//! - Content read lazily through FileSystem trait
//! - LspFileSystem intercepts reads, returns overlay or disk content
//!
//! # Critical Implementation Details
//!
//! ## The Revision Dependency Trick
//! The `source_text` tracked function MUST call `file.revision(db)` to create
//! the Salsa dependency chain. Without this, revision changes won't trigger
//! invalidation of dependent queries.
//!
//! ## StorageHandle Pattern (for tower-lsp)
//! - Database itself is NOT Send+Sync (due to RefCell in Salsa's Storage)
//! - `StorageHandle<Database>` IS Send+Sync, enabling use across threads
//! - Session stores StorageHandle, creates Database instances on-demand
//!
//! ## Why Files are in Database, Overlays in Session
//! - Files need persistent tracking across all queries (thus in Database)
//! - Overlays are LSP-specific and change frequently (thus in Session)
//! - This separation prevents Salsa invalidation cascades on every keystroke
//! - Both are accessed via `Arc<DashMap>` for thread safety and cheap cloning
//!
//! # Data Flow
//!
//! 1. **did_open/did_change** → Update overlays in Session
//! 2. **notify_file_changed()** → Bump revision, tell Salsa something changed
//! 3. **Salsa query executes** → Calls source_text()
//! 4. **source_text() calls file.revision(db)** → Creates dependency
//! 5. **source_text() calls db.read_file_content()** → Goes through FileSystem
//! 6. **LspFileSystem intercepts** → Returns overlay if exists, else disk
//! 7. **Query gets content** → Without knowing about LSP/overlays
//!
//! This design achieves:
//! - Fast overlay updates (no Salsa invalidation)
//! - Proper incremental computation (via revision tracking)
//! - Thread safety (via `Arc<DashMap>` and StorageHandle)
//! - Clean separation of concerns (LSP vs computation)

use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;

use dashmap::DashMap;

use crate::{FileKind, FileSystem};

/// Database trait that provides file system access for Salsa queries
#[salsa::db]
pub trait Db: salsa::Database {
    /// Get the file system for reading files (with overlay support)
    fn fs(&self) -> Option<Arc<dyn FileSystem>>;

    /// Read file content through the file system
    /// This is the primary way Salsa queries should read files, as it
    /// automatically checks overlays before falling back to disk.
    fn read_file_content(&self, path: &Path) -> std::io::Result<String>;
}

/// Salsa database root for workspace
///
/// The [`Database`] provides default storage and, in tests, captures Salsa events for
/// reuse/diagnostics. It serves as the core incremental computation engine, tracking
/// dependencies and invalidations across all inputs and derived queries.
///
/// The database integrates with the FileSystem abstraction to read files through
/// the LspFileSystem, which automatically checks overlays before falling back to disk.
#[salsa::db]
#[derive(Clone)]
pub struct Database {
    storage: salsa::Storage<Self>,

    /// FileSystem integration for reading files (with overlay support)
    /// This allows the database to read files through LspFileSystem, which
    /// automatically checks for overlays before falling back to disk files.
    fs: Option<Arc<dyn FileSystem>>,

    /// File tracking outside of Salsa but within Database (Arc for cheap cloning).
    /// This follows Ruff's pattern where files are tracked in the Database struct
    /// but not as part of Salsa's storage, enabling cheap clones via Arc.
    files: Arc<DashMap<PathBuf, SourceFile>>,

    // The logs are only used for testing and demonstrating reuse:
    #[cfg(test)]
    logs: Arc<Mutex<Option<Vec<String>>>>,
}

#[cfg(test)]
impl Default for Database {
    fn default() -> Self {
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
            fs: None,
            files: Arc::new(DashMap::new()),
            logs,
        }
    }
}

impl Database {
    /// Create a new database with fresh storage.
    pub fn new(file_system: Arc<dyn FileSystem>, files: Arc<DashMap<PathBuf, SourceFile>>) -> Self {
        Self {
            storage: salsa::Storage::new(None),
            fs: Some(file_system),
            files,
            #[cfg(test)]
            logs: Arc::new(Mutex::new(None)),
        }
    }

    /// Create a database instance from an existing storage.
    /// This preserves both the file system and files Arc across database operations.
    pub fn from_storage(
        storage: salsa::Storage<Self>,
        file_system: Arc<dyn FileSystem>,
        files: Arc<DashMap<PathBuf, SourceFile>>,
    ) -> Self {
        Self {
            storage,
            fs: Some(file_system),
            files,
            #[cfg(test)]
            logs: Arc::new(Mutex::new(None)),
        }
    }

    /// Read file content through the file system
    /// This is the primary way Salsa queries should read files, as it
    /// automatically checks overlays before falling back to disk.
    pub fn read_file_content(&self, path: &Path) -> std::io::Result<String> {
        if let Some(fs) = &self.fs {
            fs.read_to_string(path)
        } else {
            std::fs::read_to_string(path)
        }
    }

    /// Get or create a SourceFile for the given path.
    ///
    /// This method implements Ruff's pattern for lazy file creation. Files are created
    /// with an initial revision of 0 and tracked in the Database's `DashMap`. The `Arc`
    /// ensures cheap cloning while maintaining thread safety.
    pub fn get_or_create_file(&mut self, path: PathBuf) -> SourceFile {
        if let Some(file_ref) = self.files.get(&path) {
            // Copy the value (SourceFile is Copy) and drop the guard immediately
            let file = *file_ref;
            drop(file_ref); // Explicitly drop the guard to release the lock
            return file;
        }

        // File doesn't exist, so we need to create it
        let kind = FileKind::from_path(&path);
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

    /// Get a reference to the storage for handle extraction.
    ///
    /// This is used by Session to extract the StorageHandle after mutations.
    pub fn storage(&self) -> &salsa::Storage<Self> {
        &self.storage
    }

    /// Consume the database and return its storage.
    ///
    /// This is used when you need to take ownership of the storage.
    pub fn into_storage(self) -> salsa::Storage<Self> {
        self.storage
    }
}

#[salsa::db]
impl salsa::Database for Database {}

#[salsa::db]
impl Db for Database {
    fn fs(&self) -> Option<Arc<dyn FileSystem>> {
        self.fs.clone()
    }

    fn read_file_content(&self, path: &Path) -> std::io::Result<String> {
        match &self.fs {
            Some(fs) => fs.read_to_string(path),
            None => std::fs::read_to_string(path), // Fallback to direct disk access
        }
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

/// Read file content through the FileSystem, creating proper Salsa dependencies.
///
/// This is the CRITICAL function that implements Ruff's two-layer architecture.
/// The call to `file.revision(db)` creates a Salsa dependency, ensuring that
/// when the revision changes, this function (and all dependent queries) are
/// invalidated and re-executed.
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

/// Global input configuring ordered template loader roots.
///
/// [`TemplateLoaderOrder`] represents the Django `TEMPLATES[n]['DIRS']` configuration,
/// defining the search order for template resolution. This is a global input that
/// affects template name resolution across the entire project.
#[salsa::input]
pub struct TemplateLoaderOrder {
    /// Ordered list of template root directories
    #[returns(ref)]
    pub roots: Arc<[String]>,
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

/// Parse a Django template file by path using the file system.
///
/// This Salsa tracked function reads file content through the FileSystem, which automatically
/// checks overlays before falling back to disk, implementing Ruff's two-layer architecture.
///
/// Returns `None` for non-template files or if file cannot be read.
#[salsa::tracked]
pub fn parse_template_by_path(db: &dyn Db, file_path: FilePath) -> Option<Arc<TemplateAst>> {
    // Read file content through the FileSystem (checks overlays first)
    let path = Path::new(file_path.path(db).as_ref());
    let Ok(text) = db.read_file_content(path) else {
        return None;
    };

    // Call the pure parsing function from djls-templates
    match djls_templates::parse_template(&text) {
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

/// Get template parsing errors for a file by path.
///
/// This Salsa tracked function extracts just the errors from the parsed template,
/// useful for diagnostics without needing the full AST.
///
/// Reads files through the FileSystem for overlay support.
///
/// Returns an empty vector for non-template files.
#[salsa::tracked]
pub fn template_errors_by_path(db: &dyn Db, file_path: FilePath) -> Arc<[String]> {
    parse_template_by_path(db, file_path)
        .map_or_else(|| Arc::from(vec![]), |ast| Arc::from(ast.errors.clone()))
}

/// Get template parsing errors for a file.
///
/// This Salsa tracked function extracts just the errors from the parsed template,
/// useful for diagnostics without needing the full AST.
///
/// Returns an empty vector for non-template files.
#[salsa::tracked]
pub fn template_errors(db: &dyn Db, file: SourceFile) -> Arc<[String]> {
    parse_template(db, file).map_or_else(|| Arc::from(vec![]), |ast| Arc::from(ast.errors.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffers::Buffers;
    use crate::document::TextDocument;
    use crate::fs::WorkspaceFileSystem;
    use crate::language::LanguageId;
    use dashmap::DashMap;
    use salsa::Setter;
    use std::collections::HashMap;
    use std::io;

    // Simple in-memory filesystem for testing
    struct InMemoryFileSystem {
        files: HashMap<PathBuf, String>,
    }

    impl InMemoryFileSystem {
        fn new() -> Self {
            Self {
                files: HashMap::new(),
            }
        }

        fn add_file(&mut self, path: PathBuf, content: String) {
            self.files.insert(path, content);
        }
    }

    impl FileSystem for InMemoryFileSystem {
        fn read_to_string(&self, path: &Path) -> io::Result<String> {
            self.files
                .get(path)
                .cloned()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "File not found"))
        }

        fn exists(&self, path: &Path) -> bool {
            self.files.contains_key(path)
        }

        fn is_file(&self, path: &Path) -> bool {
            self.files.contains_key(path)
        }

        fn is_directory(&self, _path: &Path) -> bool {
            false
        }

        fn read_directory(&self, _path: &Path) -> io::Result<Vec<PathBuf>> {
            Ok(vec![])
        }

        fn metadata(&self, _path: &Path) -> io::Result<std::fs::Metadata> {
            Err(io::Error::new(io::ErrorKind::Unsupported, "Not supported"))
        }
    }

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
        let file = db.get_or_create_file(template_path.clone());

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
        let file = db.get_or_create_file(template_path.clone());

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

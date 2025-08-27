//! Salsa database and input entities for workspace.
//!
//! This module defines the Salsa world—what can be set and tracked incrementally.
//! Inputs are kept minimal to avoid unnecessary recomputation.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
#[cfg(test)]
use std::sync::Mutex;

use dashmap::DashMap;
use url::Url;

use crate::{FileId, FileKind};

/// Salsa database root for workspace
///
/// The [`Database`] provides default storage and, in tests, captures Salsa events for
/// reuse/diagnostics. It serves as the core incremental computation engine, tracking
/// dependencies and invalidations across all inputs and derived queries.
/// 
/// This database also manages the file system overlay for the workspace,
/// mapping URLs to FileIds and storing file content.
#[salsa::db]
#[derive(Clone)]
pub struct Database {
    storage: salsa::Storage<Self>,
    
    /// Map from file URL to FileId (thread-safe)
    files: DashMap<Url, FileId>,
    
    /// Map from FileId to file content (thread-safe)
    content: DashMap<FileId, Arc<str>>,
    
    /// Next FileId to allocate (thread-safe counter)
    next_file_id: Arc<AtomicU32>,

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
            files: DashMap::new(),
            content: DashMap::new(),
            next_file_id: Arc::new(AtomicU32::new(0)),
            logs,
        }
    }
}

impl Database {
    /// Create a new database instance
    pub fn new() -> Self {
        Self {
            storage: salsa::Storage::new(None),
            files: DashMap::new(),
            content: DashMap::new(),
            next_file_id: Arc::new(AtomicU32::new(0)),
            #[cfg(test)]
            logs: Arc::new(Mutex::new(None)),
        }
    }
    
    /// Create a new database instance from a storage handle.
    /// This is used by Session::db() to create databases from the StorageHandle.
    pub fn from_storage(storage: salsa::Storage<Self>) -> Self {
        Self {
            storage,
            files: DashMap::new(),
            content: DashMap::new(),
            next_file_id: Arc::new(AtomicU32::new(0)),
            #[cfg(test)]
            logs: Arc::new(Mutex::new(None)),
        }
    }
    
    /// Add or update a file in the workspace
    pub fn set_file(&mut self, url: Url, content: String, _kind: FileKind) {
        let file_id = if let Some(existing_id) = self.files.get(&url) {
            *existing_id
        } else {
            let new_id = FileId::from_raw(self.next_file_id.fetch_add(1, Ordering::SeqCst));
            self.files.insert(url.clone(), new_id);
            new_id
        };

        let content = Arc::<str>::from(content);
        self.content.insert(file_id, content.clone());

        // TODO: Update Salsa inputs here when we connect them
    }
    
    /// Remove a file from the workspace
    pub fn remove_file(&mut self, url: &Url) {
        if let Some((_, file_id)) = self.files.remove(url) {
            self.content.remove(&file_id);
            // TODO: Remove from Salsa when we connect inputs
        }
    }
    
    /// Get the content of a file by URL
    pub fn get_file_content(&self, url: &Url) -> Option<Arc<str>> {
        let file_id = self.files.get(url)?;
        self.content.get(&*file_id).map(|content| content.clone())
    }
    
    /// Get the content of a file by FileId
    pub(crate) fn get_content_by_id(&self, file_id: FileId) -> Option<Arc<str>> {
        self.content.get(&file_id).map(|content| content.clone())
    }
    
    /// Check if a file exists in the workspace
    pub fn has_file(&self, url: &Url) -> bool {
        self.files.contains_key(url)
    }
    
    /// Get all file URLs in the workspace
    pub fn files(&self) -> impl Iterator<Item = Url> + use<'_> {
        self.files.iter().map(|entry| entry.key().clone())
    }
}

#[salsa::db]
impl salsa::Database for Database {}

/// Represents a single file's classification and current content.
///
/// [`SourceFile`] is a Salsa input entity that tracks both the file's type (for routing
/// to appropriate analyzers) and its current text content. The text is stored as
/// `Arc<str>` for efficient sharing across the incremental computation graph.
#[salsa::input]
pub struct SourceFile {
    /// The file's classification for analysis routing
    pub kind: FileKind,
    /// The current text content of the file
    #[returns(ref)]
    pub text: Arc<str>,
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
/// The parse is only re-executed when the file's text content changes, enabling
/// efficient incremental template analysis.
///
/// Returns `None` for non-template files.
#[salsa::tracked]
pub fn parse_template(db: &dyn salsa::Database, file: SourceFile) -> Option<Arc<TemplateAst>> {
    // Only parse template files
    if file.kind(db) != FileKind::Template {
        return None;
    }

    let text = file.text(db);

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

/// Get template parsing errors for a file.
///
/// This Salsa tracked function extracts just the errors from the parsed template,
/// useful for diagnostics without needing the full AST.
///
/// Returns an empty vector for non-template files.
#[salsa::tracked]
pub fn template_errors(db: &dyn salsa::Database, file: SourceFile) -> Arc<[String]> {
    parse_template(db, file).map_or_else(|| Arc::from(vec![]), |ast| Arc::from(ast.errors.clone()))
}

#[cfg(test)]
mod tests {
    use salsa::Setter;

    use super::*;

    #[test]
    fn test_template_parsing_caches_result() {
        let db = Database::default();

        // Create a template file
        let template_content: Arc<str> = Arc::from("{% if user %}Hello {{ user.name }}{% endif %}");
        let file = SourceFile::new(&db, FileKind::Template, template_content.clone());

        // First parse - should execute the parsing
        let ast1 = parse_template(&db, file);
        assert!(ast1.is_some());

        // Second parse - should return cached result (same Arc)
        let ast2 = parse_template(&db, file);
        assert!(ast2.is_some());

        // Verify they're the same Arc (cached)
        assert!(Arc::ptr_eq(&ast1.unwrap(), &ast2.unwrap()));
    }

    #[test]
    fn test_template_parsing_invalidates_on_change() {
        let mut db = Database::default();

        // Create a template file
        let template_content1: Arc<str> = Arc::from("{% if user %}Hello{% endif %}");
        let file = SourceFile::new(&db, FileKind::Template, template_content1);

        // First parse
        let ast1 = parse_template(&db, file);
        assert!(ast1.is_some());

        // Change the content
        let template_content2: Arc<str> =
            Arc::from("{% for item in items %}{{ item }}{% endfor %}");
        file.set_text(&mut db).to(template_content2);

        // Parse again - should re-execute due to changed content
        let ast2 = parse_template(&db, file);
        assert!(ast2.is_some());

        // Verify they're different Arcs (re-parsed)
        assert!(!Arc::ptr_eq(&ast1.unwrap(), &ast2.unwrap()));
    }

    #[test]
    fn test_non_template_files_return_none() {
        let db = Database::default();

        // Create a Python file
        let python_content: Arc<str> = Arc::from("def hello():\n    print('Hello')");
        let file = SourceFile::new(&db, FileKind::Python, python_content);

        // Should return None for non-template files
        let ast = parse_template(&db, file);
        assert!(ast.is_none());

        // Errors should be empty for non-template files
        let errors = template_errors(&db, file);
        assert!(errors.is_empty());
    }

    #[test]
    fn test_template_errors_tracked_separately() {
        let db = Database::default();

        // Create a template with an error (unclosed tag)
        let template_content: Arc<str> = Arc::from("{% if user %}Hello {{ user.name }");
        let file = SourceFile::new(&db, FileKind::Template, template_content);

        // Get errors
        let errors1 = template_errors(&db, file);
        let errors2 = template_errors(&db, file);

        // Should be cached (same Arc)
        assert!(Arc::ptr_eq(&errors1, &errors2));
    }
}

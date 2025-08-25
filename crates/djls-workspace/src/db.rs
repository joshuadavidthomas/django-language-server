//! Salsa database and input entities for workspace.
//!
//! This module defines the Salsa world—what can be set and tracked incrementally.
//! Inputs are kept minimal to avoid unnecessary recomputation.

use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;

/// Salsa database root for workspace
///
/// The [`Database`] provides default storage and, in tests, captures Salsa events for
/// reuse/diagnostics. It serves as the core incremental computation engine, tracking
/// dependencies and invalidations across all inputs and derived queries.
#[salsa::db]
#[derive(Clone)]
#[cfg_attr(not(test), derive(Default))]
pub struct Database {
    storage: salsa::Storage<Self>,

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
            logs,
        }
    }
}

#[salsa::db]
impl salsa::Database for Database {}

/// Minimal classification for analysis routing.
///
/// [`FileKindMini`] provides a lightweight categorization of files to determine which
/// analysis pipelines should process them. This is the Salsa-side representation
/// of file types, mapped from the VFS layer's `vfs::FileKind`.
#[derive(Clone, Eq, PartialEq, Hash, Debug)]
pub enum FileKindMini {
    /// Python source file (.py)
    Python,
    /// Django template file (.html, .jinja, etc.)
    Template,
    /// Other file types not requiring specialized analysis
    Other,
}

/// Represents a single file's classification and current content.
///
/// [`SourceFile`] is a Salsa input entity that tracks both the file's type (for routing
/// to appropriate analyzers) and its current text content. The text is stored as
/// `Arc<str>` for efficient sharing across the incremental computation graph.
#[salsa::input]
pub struct SourceFile {
    /// The file's classification for analysis routing
    pub kind: FileKindMini,
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
    if file.kind(db) != FileKindMini::Template {
        return None;
    }

    let text = file.text(db);
    
    // Call the pure parsing function from djls-templates
    match djls_templates::parse_template(&text) {
        Ok((ast, errors)) => {
            // Convert errors to strings
            let error_strings = errors.into_iter().map(|e| e.to_string()).collect();
            Some(Arc::new(TemplateAst { ast, errors: error_strings }))
        },
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
    parse_template(db, file)
        .map(|ast| Arc::from(ast.errors.clone()))
        .unwrap_or_else(|| Arc::from(vec![]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use salsa::Setter;

    #[test]
    fn test_template_parsing_caches_result() {
        let db = Database::default();
        
        // Create a template file
        let template_content: Arc<str> = Arc::from("{% if user %}Hello {{ user.name }}{% endif %}");
        let file = SourceFile::new(&db, FileKindMini::Template, template_content.clone());
        
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
        let file = SourceFile::new(&db, FileKindMini::Template, template_content1);
        
        // First parse
        let ast1 = parse_template(&db, file);
        assert!(ast1.is_some());
        
        // Change the content
        let template_content2: Arc<str> = Arc::from("{% for item in items %}{{ item }}{% endfor %}");
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
        let file = SourceFile::new(&db, FileKindMini::Python, python_content);
        
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
        let file = SourceFile::new(&db, FileKindMini::Template, template_content);
        
        // Get errors
        let errors1 = template_errors(&db, file);
        let errors2 = template_errors(&db, file);
        
        // Should be cached (same Arc)
        assert!(Arc::ptr_eq(&errors1, &errors2));
    }
}

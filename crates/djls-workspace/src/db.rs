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

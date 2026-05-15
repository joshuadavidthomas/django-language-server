//! Project-specific database capabilities.
//!
//! The trait exposes the runtime state that project-aware semantic code needs:
//! the current `Project` input and the project introspector. Imperative refresh
//! workflows live in sibling modules as free functions so the trait stays a
//! capability boundary rather than a service object.

use std::sync::Arc;

use camino::Utf8PathBuf;

use crate::project::introspector::ProjectIntrospector;
use crate::project::Project;

/// Project-specific database capabilities.
#[salsa::db]
pub trait Db: salsa::Database {
    /// Get the current project (if set)
    fn project(&self) -> Option<Project>;

    /// Get the shared project introspector.
    fn project_introspector(&self) -> Arc<ProjectIntrospector>;
}

/// Return the current project root or fall back to the current working directory.
#[must_use]
pub fn project_root_or_cwd(db: &dyn Db) -> Utf8PathBuf {
    if let Some(project) = db.project() {
        project.root(db).clone()
    } else if let Ok(current_dir) = std::env::current_dir() {
        Utf8PathBuf::from_path_buf(current_dir).unwrap_or_else(|_| Utf8PathBuf::from("."))
    } else {
        Utf8PathBuf::from(".")
    }
}

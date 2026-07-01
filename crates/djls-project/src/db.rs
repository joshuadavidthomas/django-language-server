//! Project-specific database capabilities.
//!
//! The trait exposes the runtime state that project-aware semantic code needs:
//! the current `Project` input. Imperative synchronization lives outside the
//! trait so it stays a capability boundary rather than a service object.

use camino::Utf8PathBuf;
use djls_source::Db as SourceDb;

use crate::Project;

/// Project-specific database capabilities.
#[salsa::db]
pub trait Db: SourceDb {
    /// Get the current project (if set)
    fn project(&self) -> Option<Project>;

    /// Return the current project root or fall back to the current working directory.
    fn project_root_or_cwd(&self) -> Utf8PathBuf {
        if let Some(project) = self.project() {
            project.root(self).clone()
        } else if let Ok(current_dir) = std::env::current_dir() {
            Utf8PathBuf::from_path_buf(current_dir).unwrap_or_else(|_| Utf8PathBuf::from("."))
        } else {
            Utf8PathBuf::from(".")
        }
    }
}

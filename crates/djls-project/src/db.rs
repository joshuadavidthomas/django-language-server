//! Project-specific database trait and queries.
//!
//! This module extends the workspace database trait with project-specific
//! functionality including metadata access and Python environment discovery.
//!
//! ## Architecture
//!
//! Following the Salsa pattern established in workspace and templates crates:
//! - `DjangoProject` is a Salsa input representing external project state
//! - Tracked functions compute derived values (Python env, Django config)
//! - Database trait provides stable configuration (metadata, template tags)

use std::sync::Arc;

use camino::Utf8PathBuf;

use crate::inspector::Inspector;
use crate::project::Project;

/// Project-specific database trait extending the workspace database
#[salsa::db]
pub trait Db: salsa::Database {
    /// Get the current project (if set)
    fn project(&self) -> Option<Project>;

    /// Get the shared inspector for executing Python queries
    fn inspector(&self) -> Arc<Inspector>;

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

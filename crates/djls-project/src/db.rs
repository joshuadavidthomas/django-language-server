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

use std::path::Path;
use std::sync::Arc;

use djls_workspace::Db as WorkspaceDb;

use crate::inspector::pool::InspectorPool;
use crate::project::Project;

/// Project-specific database trait extending the workspace database
#[salsa::db]
pub trait Db: WorkspaceDb {
    /// Get the current project (if set)
    fn project(&self) -> Option<Project>;

    /// Get the shared inspector pool for executing Python queries
    fn inspector_pool(&self) -> Arc<InspectorPool>;

    /// Get the project root path if a project is set
    fn project_path(&self) -> Option<&Path> {
        self.project().map(|p| p.root(self).as_path())
    }
}

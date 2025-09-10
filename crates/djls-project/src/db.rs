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

use crate::django::TemplateTags;
use crate::meta::Project;
use crate::meta::ProjectMetadata;

/// Project-specific database trait extending the workspace database
#[salsa::db]
pub trait Db: WorkspaceDb {
    /// Get the project metadata containing root path and venv configuration
    fn metadata(&self) -> &ProjectMetadata;

    /// Get discovered template tags for the project (if available).
    /// This is populated by the LSP server after querying Django.
    fn template_tags(&self) -> Option<Arc<TemplateTags>>;

    /// Get or create a Project input for a given path
    fn project(&self, root: &Path) -> Project;
}

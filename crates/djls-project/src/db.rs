//! Project-specific database trait and queries.
//!
//! This module extends the workspace database trait with project-specific
//! functionality including metadata access and Python environment discovery.

use djls_workspace::Db as WorkspaceDb;

use crate::meta::ProjectMetadata;
use crate::python::PythonEnvironment;

/// Project-specific database trait extending the workspace database
#[salsa::db]
pub trait Db: WorkspaceDb {
    /// Get the project metadata containing root path and venv configuration
    fn metadata(&self) -> &ProjectMetadata;
}

/// Find the Python environment for the project.
///
/// This Salsa tracked function discovers the Python environment based on:
/// 1. Explicit venv path from metadata
/// 2. VIRTUAL_ENV environment variable
/// 3. Common venv directories in project root (.venv, venv, env, .env)
/// 4. System Python as fallback
#[salsa::tracked]
pub fn find_python_environment(db: &dyn Db) -> Option<PythonEnvironment> {
    let project_path = db.metadata().root().as_path();
    let venv_path = db.metadata().venv().and_then(|p| p.to_str());

    PythonEnvironment::new(project_path, venv_path)
}

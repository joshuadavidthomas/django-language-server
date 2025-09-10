use std::path::PathBuf;

use crate::python::Interpreter;

/// Complete project configuration as a Salsa input.
///
/// Following Ruff's pattern, this contains all external project configuration
/// rather than minimal keys that everything derives from. This replaces both
/// Project input.
#[salsa::input]
#[derive(Debug)]
pub struct Project {
    /// The project root path
    #[returns(ref)]
    pub root: String,
    /// Interpreter specification for Python environment discovery
    pub interpreter: Interpreter,
    /// Optional Django settings module override from configuration
    #[returns(ref)]
    pub settings_module: Option<String>,
    /// Revision number for invalidation tracking
    pub revision: u64,
}

#[derive(Clone, Debug)]
pub struct ProjectMetadata {
    root: PathBuf,
    venv: Option<PathBuf>,
}

impl ProjectMetadata {
    #[must_use]
    pub fn new(root: PathBuf, venv: Option<PathBuf>) -> Self {
        ProjectMetadata { root, venv }
    }

    #[must_use]
    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    #[must_use]
    pub fn venv(&self) -> Option<&PathBuf> {
        self.venv.as_ref()
    }
}

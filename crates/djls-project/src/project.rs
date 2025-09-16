use camino::Utf8PathBuf;

use crate::db::Db as ProjectDb;
use crate::django_available;
use crate::django_settings_module;
use crate::get_templatetags;
use crate::python::Interpreter;

/// Complete project configuration as a Salsa input.
///
/// Following Ruff's pattern, this contains all external project configuration
/// rather than minimal keys that everything derives from. This replaces both
/// Project input and ProjectMetadata.
// TODO: Add templatetags as a field on this input
#[salsa::input]
#[derive(Debug)]
pub struct Project {
    /// The project root path
    #[returns(ref)]
    pub root: Utf8PathBuf,
    /// Interpreter specification for Python environment discovery
    pub interpreter: Interpreter,
    /// Optional Django settings module override from configuration
    #[returns(ref)]
    pub settings_module: Option<String>,
}

impl Project {
    pub fn initialize(self, db: &dyn ProjectDb) {
        let _ = django_available(db, self);
        let _ = django_settings_module(db, self);
        let _ = get_templatetags(db, self);
    }
}

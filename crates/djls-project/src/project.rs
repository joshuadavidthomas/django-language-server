use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::Settings;

use crate::db::Db as ProjectDb;
use crate::django_available;
use crate::django_settings_module;
use crate::python::Interpreter;
use crate::templatetags;

/// Complete project configuration as a Salsa input.
///
/// Following Ruff's pattern, this contains all external project configuration
/// rather than minimal keys that everything derives from. This replaces both
/// Project input and ProjectMetadata, and now captures the resolved `djls` settings
/// so higher layers can access configuration through Salsa instead of rereading
/// from disk.
#[salsa::input]
#[derive(Debug)]
pub struct Project {
    /// The project root path
    #[returns(ref)]
    pub root: Utf8PathBuf,
    /// Interpreter specification for Python environment discovery
    pub interpreter: Interpreter,
    /// Resolved djls configuration for this project
    #[returns(ref)]
    pub settings: Settings,
}

impl Project {
    pub fn bootstrap(
        db: &dyn ProjectDb,
        root: &Utf8Path,
        venv_path: Option<&str>,
        settings: Settings,
    ) -> Project {
        let interpreter = venv_path
            .map(|path| Interpreter::VenvPath(path.to_string()))
            .or_else(|| std::env::var("VIRTUAL_ENV").ok().map(Interpreter::VenvPath))
            .unwrap_or(Interpreter::Auto);

        Project::new(db, root.to_path_buf(), interpreter, settings)
    }

    pub fn initialize(self, db: &dyn ProjectDb) {
        let _ = django_available(db, self);
        let _ = django_settings_module(db, self);
        let _ = templatetags(db, self);
    }
}

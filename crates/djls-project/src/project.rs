use camino::Utf8Path;
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
    pub fn bootstrap(
        db: &dyn ProjectDb,
        root: &Utf8Path,
        venv_path: Option<&str>,
        settings_module: Option<&str>,
    ) -> Project {
        let interpreter = venv_path
            .map(|path| Interpreter::VenvPath(path.to_string()))
            .or_else(|| std::env::var("VIRTUAL_ENV").ok().map(Interpreter::VenvPath))
            .unwrap_or(Interpreter::Auto);

        let django_settings = settings_module
            .map(std::string::ToString::to_string)
            .or_else(|| std::env::var("DJANGO_SETTINGS_MODULE").ok());

        Project::new(db, root.to_path_buf(), interpreter, django_settings)
    }

    pub fn initialize(self, db: &dyn ProjectDb) {
        let _ = django_available(db, self);
        let _ = django_settings_module(db, self);
        let _ = get_templatetags(db, self);
    }
}

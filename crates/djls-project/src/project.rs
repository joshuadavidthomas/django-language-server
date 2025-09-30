use camino::Utf8Path;
use camino::Utf8PathBuf;

use crate::db::Db as ProjectDb;
use crate::django_available;
use crate::template_dirs;
use crate::templatetags;
use crate::Interpreter;

/// Complete project configuration as a Salsa input.
///
/// This represents the core identity of a project: where it is (root path),
/// which Python environment to use (interpreter), and Django-specific configuration.
#[salsa::input]
#[derive(Debug)]
pub struct Project {
    /// The project root path
    #[returns(ref)]
    pub root: Utf8PathBuf,
    /// Interpreter specification for Python environment discovery
    #[returns(ref)]
    pub interpreter: Interpreter,
    /// Django settings module (e.g., "myproject.settings")
    #[returns(ref)]
    pub django_settings_module: Option<String>,
}

impl Project {
    pub fn bootstrap(
        db: &dyn ProjectDb,
        root: &Utf8Path,
        venv_path: Option<&str>,
        django_settings_module: Option<&str>,
    ) -> Project {
        let interpreter = Interpreter::discover(venv_path);

        let resolved_django_settings_module = django_settings_module
            .map(String::from)
            .or_else(|| {
                // Check environment variable if not configured
                std::env::var("DJANGO_SETTINGS_MODULE")
                    .ok()
                    .filter(|s| !s.is_empty())
            })
            .or_else(|| {
                // Auto-detect from project structure
                if root.join("manage.py").exists() {
                    // Look for common settings modules
                    for candidate in &["settings", "config.settings", "project.settings"] {
                        let parts: Vec<&str> = candidate.split('.').collect();
                        let mut path = root.to_path_buf();
                        for part in &parts[..parts.len() - 1] {
                            path = path.join(part);
                        }
                        if let Some(last) = parts.last() {
                            path = path.join(format!("{last}.py"));
                        }

                        if path.exists() {
                            tracing::info!("Auto-detected Django settings module: {}", candidate);
                            return Some((*candidate).to_string());
                        }
                    }
                    tracing::warn!(
                        "manage.py found but could not auto-detect Django settings module"
                    );
                } else {
                    tracing::debug!("No manage.py found, skipping Django settings auto-detection");
                }
                None
            });

        Project::new(
            db,
            root.to_path_buf(),
            interpreter,
            resolved_django_settings_module,
        )
    }

    pub fn initialize(self, db: &dyn ProjectDb) {
        let _ = django_available(db, self);
        let _ = templatetags(db, self);
        let _ = template_dirs(db, self);
    }
}

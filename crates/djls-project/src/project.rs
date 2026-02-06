use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::DiagnosticsConfig;
use djls_conf::Settings;
use djls_extraction::ExtractionResult;
use rustc_hash::FxHashMap;

use crate::db::Db as ProjectDb;
use crate::django_available;
use crate::template_dirs;
use crate::templatetags;
use crate::Interpreter;
use crate::TemplateTags;

/// Complete project configuration as a Salsa input.
///
/// This represents the core identity of a project: where it is (root path),
/// which Python environment to use (interpreter), Django-specific configuration,
/// and external data sources that drive semantic analysis.
///
/// `DiagnosticsConfig` is stored here as a config document. Tracked queries in
/// `djls-server` convert extraction results into semantic types (`TagSpecs`).
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
    /// Additional Python import paths (PYTHONPATH entries)
    #[returns(ref)]
    pub pythonpath: Vec<String>,
    /// Inspector inventory from Python subprocess (populated by `refresh_inspector`)
    #[returns(ref)]
    pub inspector_inventory: Option<TemplateTags>,
    /// Extraction results from external modules (site-packages), keyed by
    /// registration module path (e.g., `"django.templatetags.i18n"`).
    /// Populated by `refresh_inspector`. Workspace files use tracked queries
    /// via `collect_workspace_extraction_results` instead.
    #[returns(ref)]
    pub extracted_external_rules: FxHashMap<String, ExtractionResult>,
    /// Diagnostic severity configuration
    #[returns(ref)]
    pub diagnostics: DiagnosticsConfig,
}

impl Project {
    pub fn bootstrap(db: &dyn ProjectDb, root: &Utf8Path, settings: &Settings) -> Project {
        let interpreter = Interpreter::discover(settings.venv_path());

        let resolved_django_settings_module = settings
            .django_settings_module()
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
            settings.pythonpath().to_vec(),
            None,
            FxHashMap::default(),
            settings.diagnostics().clone(),
        )
    }

    pub fn initialize(self, db: &dyn ProjectDb) {
        let _ = django_available(db, self);
        let _ = templatetags(db, self);
        let _ = template_dirs(db, self);
    }
}

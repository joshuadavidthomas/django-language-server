use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::DiagnosticsConfig;
use djls_conf::Settings;
use djls_conf::TagSpecDef;
use djls_python::ExtractionResult;
use rustc_hash::FxHashMap;

use crate::db::Db as ProjectDb;
use crate::django_available;
use crate::template_dirs;
use crate::Interpreter;
use crate::TemplateLibraries;

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
    /// Manual TagSpecs configuration from TOML (fallback for extraction gaps)
    #[returns(ref)]
    pub tagspecs: TagSpecDef,
    /// Template libraries and symbols for this project.
    ///
    /// This value always exists to support progressive enhancement:
    /// - Discovered libraries are populated by scanning `sys.path`.
    /// - Installed libraries/symbols are populated by querying the Django inspector.
    ///
    /// The semantic layer combines this with `{% load %}` scope computed from templates.
    #[returns(ref)]
    pub template_libraries: TemplateLibraries,
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
        let resolved_django_settings_module = resolve_django_settings(root, settings);

        Project::new(
            db,
            root.to_path_buf(),
            interpreter,
            resolved_django_settings_module,
            settings.pythonpath().to_vec(),
            settings.tagspecs().clone(),
            TemplateLibraries::default(),
            FxHashMap::default(),
            settings.diagnostics().clone(),
        )
    }

    pub fn initialize(self, db: &dyn ProjectDb) {
        let _ = django_available(db, self);
        let _ = template_dirs(db, self);
    }
}

fn resolve_django_settings(root: &Utf8Path, settings: &Settings) -> Option<String> {
    settings
        .django_settings_module()
        .map(String::from)
        .or_else(|| {
            std::env::var("DJANGO_SETTINGS_MODULE")
                .ok()
                .filter(|s| !s.is_empty())
        })
        .or_else(|| auto_detect_settings_module(root))
}

fn auto_detect_settings_module(root: &Utf8Path) -> Option<String> {
    if !root.join("manage.py").exists() {
        tracing::debug!("No manage.py found, skipping Django settings auto-detection");
        return None;
    }

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

    tracing::warn!("manage.py found but could not auto-detect Django settings module");
    None
}

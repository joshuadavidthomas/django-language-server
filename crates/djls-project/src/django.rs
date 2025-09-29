mod templatetags;

pub use templatetags::get_templatetags;
pub use templatetags::TemplateTags;

use crate::db::Db as ProjectDb;
use crate::inspector::inspector_run;
use crate::inspector::Query;
use crate::python::python_environment;
use crate::Project;

/// Check if Django is available for the current project.
///
/// This determines if Django is installed and configured in the Python environment.
/// First consults the inspector, then falls back to environment detection.
#[salsa::tracked]
pub fn django_available(db: &dyn ProjectDb, project: Project) -> bool {
    // First try to get Django availability from inspector
    if let Some(json_data) = inspector_run(db, Query::DjangoInit) {
        // Parse the JSON response - expect a boolean
        if let Ok(available) = serde_json::from_str::<bool>(&json_data) {
            return available;
        }
    }

    // Fallback to environment detection
    python_environment(db, project).is_some()
}

/// Get the Django settings module name for the current project.
///
/// Returns the inspector result, `DJANGO_SETTINGS_MODULE` env var, or attempts to detect it
/// via common patterns.
#[salsa::tracked]
pub fn django_settings_module(db: &dyn ProjectDb, project: Project) -> Option<String> {
    // Try to get settings module from inspector
    if let Some(json_data) = inspector_run(db, Query::DjangoInit) {
        // Parse the JSON response - expect a string
        if let Ok(settings) = serde_json::from_str::<String>(&json_data) {
            return Some(settings);
        }
    }

    // Fall back to environment override if present
    if let Ok(env_value) = std::env::var("DJANGO_SETTINGS_MODULE") {
        if !env_value.is_empty() {
            return Some(env_value);
        }
    }

    let project_path = project.root(db);

    // Try to detect settings module
    if project_path.join("manage.py").exists() {
        // Look for common settings modules
        for candidate in &["settings", "config.settings", "project.settings"] {
            let parts: Vec<&str> = candidate.split('.').collect();
            let mut path = project_path.clone();
            for part in &parts[..parts.len() - 1] {
                path = path.join(part);
            }
            if let Some(last) = parts.last() {
                path = path.join(format!("{last}.py"));
            }

            if path.exists() {
                return Some((*candidate).to_string());
            }
        }
    }

    None
}

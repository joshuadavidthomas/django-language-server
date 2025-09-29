use std::ops::Deref;

use serde::Deserialize;
use serde::Serialize;

use crate::db::Db as ProjectDb;
use crate::inspector;
use crate::inspector::InspectorRequest;
use crate::python::python_environment;
use crate::Project;

#[derive(Serialize)]
struct DjangoInitRequest;

#[derive(Deserialize)]
struct DjangoInitResponse;

impl InspectorRequest for DjangoInitRequest {
    const NAME: &'static str = "django_init";
    type Response = DjangoInitResponse;
}

/// Initialize Django for the current project.
///
/// This tracked function attempts to initialize Django via the inspector.
/// Returns true if Django was successfully initialized, false otherwise.
#[salsa::tracked]
pub fn django_initialized(db: &dyn ProjectDb, _project: Project) -> bool {
    inspector::query(db, &DjangoInitRequest).is_some()
}

/// Check if Django is available for the current project.
///
/// This determines if Django is installed and configured in the Python environment.
/// First attempts to initialize Django, then falls back to environment detection.
#[salsa::tracked]
pub fn django_available(db: &dyn ProjectDb, project: Project) -> bool {
    // Try to initialize Django
    if django_initialized(db, project) {
        return true;
    }

    // Fallback to environment detection
    python_environment(db, project).is_some()
}

/// Get the Django settings module name for the current project.
///
/// Returns `DJANGO_SETTINGS_MODULE` env var, or attempts to detect it
/// via common patterns.
#[salsa::tracked]
pub fn django_settings_module(db: &dyn ProjectDb, project: Project) -> Option<String> {
    // Note: The django_init query doesn't return the settings module,
    // it just initializes Django. So we detect it ourselves.

    // Check environment override first
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

#[derive(Serialize)]
struct TemplatetagsRequest;

#[derive(Deserialize)]
struct TemplatetagsResponse {
    templatetags: Vec<TemplateTag>,
}

impl InspectorRequest for TemplatetagsRequest {
    const NAME: &'static str = "templatetags";
    type Response = TemplatetagsResponse;
}

/// Get template tags for the current project by querying the inspector.
///
/// This is the primary Salsa-tracked entry point for templatetags.
#[salsa::tracked]
pub fn templatetags(db: &dyn ProjectDb, _project: Project) -> Option<TemplateTags> {
    let response = inspector::query(db, &TemplatetagsRequest)?;
    Some(TemplateTags(response.templatetags))
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct TemplateTags(Vec<TemplateTag>);

impl Deref for TemplateTags {
    type Target = Vec<TemplateTag>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TemplateTag {
    name: String,
    module: String,
    doc: Option<String>,
}

impl TemplateTag {
    pub fn name(&self) -> &String {
        &self.name
    }

    pub fn module(&self) -> &String {
        &self.module
    }

    pub fn doc(&self) -> Option<&String> {
        self.doc.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_template_tag_fields() {
        // Test that TemplateTag fields are accessible correctly
        let tag = TemplateTag {
            name: "test_tag".to_string(),
            module: "test_module".to_string(),
            doc: Some("Test documentation".to_string()),
        };
        assert_eq!(tag.name(), "test_tag");
        assert_eq!(tag.module(), "test_module");
        assert_eq!(tag.doc(), Some(&"Test documentation".to_string()));
    }

    #[test]
    fn test_template_tags_deref() {
        // Test that TemplateTags derefs to Vec<TemplateTag>
        let tags = TemplateTags(vec![
            TemplateTag {
                name: "tag1".to_string(),
                module: "module1".to_string(),
                doc: None,
            },
            TemplateTag {
                name: "tag2".to_string(),
                module: "module2".to_string(),
                doc: None,
            },
        ]);
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0].name(), "tag1");
        assert_eq!(tags[1].name(), "tag2");
    }
}

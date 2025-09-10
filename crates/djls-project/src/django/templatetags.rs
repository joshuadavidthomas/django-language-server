use std::ops::Deref;

use anyhow::Context;
use anyhow::Result;
use serde_json::Value;

use crate::db::Db as ProjectDb;
use crate::inspector::inspector_run;
use crate::inspector::queries::InspectorQueryKind;
use crate::meta::Project;

/// Get template tags for a project by querying the inspector.
///
/// This tracked function calls the inspector to retrieve Django template tags
/// and parses the JSON response into a TemplateTags struct.
#[salsa::tracked]
pub fn template_tags(db: &dyn ProjectDb, project: Project) -> Option<TemplateTags> {
    let json_str = inspector_run(db, project, InspectorQueryKind::TemplateTags)?;

    // Parse the JSON string into a Value first
    let json_value: serde_json::Value = match serde_json::from_str(&json_str) {
        Ok(value) => value,
        Err(_) => return None,
    };

    // Parse the JSON data into TemplateTags
    TemplateTags::from_json(&json_value).ok()
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct TemplateTags(Vec<TemplateTag>);

impl Deref for TemplateTags {
    type Target = Vec<TemplateTag>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl TemplateTags {
    pub fn from_json(data: &Value) -> Result<TemplateTags> {
        let mut tags = Vec::new();

        // Parse the JSON response from the inspector
        let templatetags = data
            .get("templatetags")
            .context("Missing 'templatetags' field in response")?
            .as_array()
            .context("'templatetags' field is not an array")?;

        for tag_data in templatetags {
            let name = tag_data
                .get("name")
                .and_then(|v| v.as_str())
                .context("Missing or invalid 'name' field")?
                .to_string();

            let module = tag_data
                .get("module")
                .and_then(|v| v.as_str())
                .context("Missing or invalid 'module' field")?;

            // Extract library name from module (e.g., "django.templatetags.static" -> "static")
            let library = module
                .split('.')
                .filter(|part| part.contains("templatetags"))
                .nth(1)
                .or_else(|| module.split('.').next_back())
                .unwrap_or("builtins")
                .to_string();

            let doc = tag_data
                .get("doc")
                .and_then(|v| v.as_str())
                .map(String::from);

            tags.push(TemplateTag::new(name, library, doc));
        }

        Ok(TemplateTags(tags))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateTag {
    name: String,
    library: String,
    doc: Option<String>,
}

impl TemplateTag {
    fn new(name: String, library: String, doc: Option<String>) -> Self {
        Self { name, library, doc }
    }

    pub fn name(&self) -> &String {
        &self.name
    }

    pub fn library(&self) -> &String {
        &self.library
    }

    pub fn doc(&self) -> Option<&String> {
        self.doc.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_template_tags_parsing() {
        // Test that TemplateTags can parse valid JSON
        let json_data = r#"{
            "templatetags": [
                {
                    "name": "test_tag",
                    "module": "test_module",
                    "doc": "Test documentation"
                }
            ]
        }"#;

        let value: serde_json::Value = serde_json::from_str(json_data).unwrap();
        let tags = TemplateTags::from_json(&value).unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].name(), "test_tag");
        assert_eq!(tags[0].library(), "test_module");
        assert_eq!(tags[0].doc(), Some(&"Test documentation".to_string()));
    }
}

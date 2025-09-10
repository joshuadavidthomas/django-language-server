use std::ops::Deref;

use anyhow::Context;
use anyhow::Result;
use serde_json::Value;

#[derive(Debug, Default, Clone)]
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

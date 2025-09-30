use std::ops::Deref;

use camino::Utf8PathBuf;
use serde::Deserialize;
use serde::Serialize;

use crate::db::Db as ProjectDb;
use crate::inspector;
use crate::inspector::InspectorRequest;
use crate::Project;

#[derive(Serialize)]
struct DjangoInitRequest;

#[derive(Deserialize)]
struct DjangoInitResponse;

impl InspectorRequest for DjangoInitRequest {
    const NAME: &'static str = "django_init";
    type Response = DjangoInitResponse;
}

/// Check if Django is available for the current project.
///
/// This tracked function attempts to initialize Django via the inspector.
/// Returns true if Django was successfully initialized, false otherwise.
#[salsa::tracked]
pub fn django_available(db: &dyn ProjectDb, _project: Project) -> bool {
    inspector::query(db, &DjangoInitRequest).is_some()
}

#[derive(Serialize)]
struct TemplateDirsRequest;

#[derive(Deserialize)]
struct TemplateDirsResponse {
    dirs: Vec<Utf8PathBuf>,
}

impl InspectorRequest for TemplateDirsRequest {
    const NAME: &'static str = "template_dirs";
    type Response = TemplateDirsResponse;
}

#[salsa::tracked]
pub fn template_dirs(db: &dyn ProjectDb, _project: Project) -> Option<TemplateDirs> {
    tracing::debug!("Requesting template directories from inspector");

    let response = inspector::query(db, &TemplateDirsRequest)?;

    let dir_count = response.dirs.len();
    tracing::info!(
        "Retrieved {} template directories from inspector",
        dir_count
    );

    for (i, dir) in response.dirs.iter().enumerate() {
        tracing::debug!("  Template dir [{}]: {}", i, dir);
    }

    let missing_dirs: Vec<_> = response.dirs.iter().filter(|dir| !dir.exists()).collect();

    if !missing_dirs.is_empty() {
        tracing::warn!(
            "Found {} non-existent template directories: {:?}",
            missing_dirs.len(),
            missing_dirs
        );
    }

    Some(response.dirs)
}

type TemplateDirs = Vec<Utf8PathBuf>;

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
    let tag_count = response.templatetags.len();
    tracing::debug!("Retrieved {} templatetags from inspector", tag_count);
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

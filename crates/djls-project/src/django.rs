use std::collections::HashMap;
use std::ops::Deref;

use camino::Utf8PathBuf;
use serde::Deserialize;
use serde::Serialize;

use crate::db::Db as ProjectDb;
use crate::inspector;
use crate::inspector::InspectorRequest;
use crate::Project;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TagProvenance {
    Library {
        load_name: String,
        module: String,
    },
    Builtin {
        module: String,
    },
}

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
pub struct TemplatetagsRequest;

#[derive(Deserialize)]
pub struct TemplatetagsResponse {
    pub templatetags: Vec<TemplateTag>,
    pub libraries: HashMap<String, String>,
    pub builtins: Vec<String>,
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
    Some(TemplateTags {
        tags: response.templatetags,
        libraries: response.libraries,
        builtins: response.builtins,
    })
}

#[derive(Debug, Default, Clone, PartialEq, Deserialize)]
pub struct TemplateTags {
    tags: Vec<TemplateTag>,
    libraries: HashMap<String, String>,
    builtins: Vec<String>,
}

impl TemplateTags {
    #[must_use]
    pub fn new(
        tags: Vec<TemplateTag>,
        libraries: HashMap<String, String>,
        builtins: Vec<String>,
    ) -> Self {
        Self {
            tags,
            libraries,
            builtins,
        }
    }

    /// Construct a `TemplateTags` from a raw inspector response.
    #[must_use]
    pub fn from_response(response: TemplatetagsResponse) -> Self {
        Self {
            tags: response.templatetags,
            libraries: response.libraries,
            builtins: response.builtins,
        }
    }

    #[must_use]
    pub fn tags(&self) -> &[TemplateTag] {
        &self.tags
    }

    #[must_use]
    pub fn libraries(&self) -> &HashMap<String, String> {
        &self.libraries
    }

    #[must_use]
    pub fn builtins(&self) -> &[String] {
        &self.builtins
    }

    pub fn iter(&self) -> impl Iterator<Item = &TemplateTag> {
        self.tags.iter()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.tags.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tags.is_empty()
    }
}

impl Deref for TemplateTags {
    type Target = Vec<TemplateTag>;

    fn deref(&self) -> &Self::Target {
        &self.tags
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TemplateTag {
    name: String,
    provenance: TagProvenance,
    defining_module: String,
    doc: Option<String>,
}

impl TemplateTag {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn provenance(&self) -> &TagProvenance {
        &self.provenance
    }

    #[must_use]
    pub fn defining_module(&self) -> &str {
        &self.defining_module
    }

    #[must_use]
    pub fn registration_module(&self) -> &str {
        match &self.provenance {
            TagProvenance::Library { module, .. } | TagProvenance::Builtin { module } => module,
        }
    }

    #[must_use]
    pub fn library_load_name(&self) -> Option<&str> {
        match &self.provenance {
            TagProvenance::Library { load_name, .. } => Some(load_name),
            TagProvenance::Builtin { .. } => None,
        }
    }

    #[must_use]
    pub fn is_builtin(&self) -> bool {
        matches!(self.provenance, TagProvenance::Builtin { .. })
    }

    #[must_use]
    pub fn doc(&self) -> Option<&str> {
        self.doc.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builtin_tag(name: &str, module: &str, defining: &str) -> TemplateTag {
        TemplateTag {
            name: name.to_string(),
            provenance: TagProvenance::Builtin {
                module: module.to_string(),
            },
            defining_module: defining.to_string(),
            doc: None,
        }
    }

    fn library_tag(name: &str, load_name: &str, module: &str, defining: &str) -> TemplateTag {
        TemplateTag {
            name: name.to_string(),
            provenance: TagProvenance::Library {
                load_name: load_name.to_string(),
                module: module.to_string(),
            },
            defining_module: defining.to_string(),
            doc: None,
        }
    }

    #[test]
    fn test_tag_provenance_deserialize_builtin() {
        let json = r#"{"builtin": {"module": "django.template.defaulttags"}}"#;
        let provenance: TagProvenance = serde_json::from_str(json).unwrap();
        assert_eq!(
            provenance,
            TagProvenance::Builtin {
                module: "django.template.defaulttags".to_string()
            }
        );
    }

    #[test]
    fn test_tag_provenance_deserialize_library() {
        let json =
            r#"{"library": {"load_name": "static", "module": "django.templatetags.static"}}"#;
        let provenance: TagProvenance = serde_json::from_str(json).unwrap();
        assert_eq!(
            provenance,
            TagProvenance::Library {
                load_name: "static".to_string(),
                module: "django.templatetags.static".to_string()
            }
        );
    }

    #[test]
    fn test_template_tag_deserialize() {
        let json = r#"{
            "name": "block",
            "provenance": {"builtin": {"module": "django.template.defaulttags"}},
            "defining_module": "django.template.loader_tags",
            "doc": "Define a block"
        }"#;
        let tag: TemplateTag = serde_json::from_str(json).unwrap();
        assert_eq!(tag.name(), "block");
        assert_eq!(tag.defining_module(), "django.template.loader_tags");
        assert_eq!(
            tag.registration_module(),
            "django.template.defaulttags"
        );
        assert!(tag.is_builtin());
        assert_eq!(tag.library_load_name(), None);
        assert_eq!(tag.doc(), Some("Define a block"));
    }

    #[test]
    fn test_template_tag_library_accessors() {
        let tag = library_tag(
            "static",
            "static",
            "django.templatetags.static",
            "django.templatetags.static",
        );
        assert_eq!(tag.name(), "static");
        assert!(!tag.is_builtin());
        assert_eq!(tag.library_load_name(), Some("static"));
        assert_eq!(tag.registration_module(), "django.templatetags.static");
        assert_eq!(tag.defining_module(), "django.templatetags.static");
    }

    #[test]
    fn test_template_tag_builtin_accessors() {
        let tag = builtin_tag("if", "django.template.defaulttags", "django.template.defaulttags");
        assert_eq!(tag.name(), "if");
        assert!(tag.is_builtin());
        assert_eq!(tag.library_load_name(), None);
        assert_eq!(
            tag.registration_module(),
            "django.template.defaulttags"
        );
    }

    #[test]
    fn test_template_tags_registry_accessors() {
        let mut libraries = HashMap::new();
        libraries.insert(
            "static".to_string(),
            "django.templatetags.static".to_string(),
        );
        libraries.insert("i18n".to_string(), "django.templatetags.i18n".to_string());

        let tags = TemplateTags {
            tags: vec![
                builtin_tag("if", "django.template.defaulttags", "django.template.defaulttags"),
                library_tag(
                    "get_static_prefix",
                    "static",
                    "django.templatetags.static",
                    "django.templatetags.static",
                ),
            ],
            libraries,
            builtins: vec![
                "django.template.defaulttags".to_string(),
                "django.template.defaultfilters".to_string(),
            ],
        };

        assert_eq!(tags.len(), 2);
        assert!(!tags.is_empty());
        assert_eq!(tags.libraries().len(), 2);
        assert_eq!(
            tags.libraries().get("static"),
            Some(&"django.templatetags.static".to_string())
        );
        assert_eq!(tags.builtins().len(), 2);
        assert_eq!(tags.builtins()[0], "django.template.defaulttags");
    }

    #[test]
    fn test_template_tags_deref() {
        let tags = TemplateTags {
            tags: vec![
                builtin_tag("tag1", "module1", "module1"),
                builtin_tag("tag2", "module2", "module2"),
            ],
            libraries: HashMap::new(),
            builtins: vec![],
        };
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0].name(), "tag1");
        assert_eq!(tags[1].name(), "tag2");
    }
}

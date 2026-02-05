use std::collections::HashMap;

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
    libraries: HashMap<String, String>,
    builtins: Vec<String>,
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
    Some(TemplateTags {
        libraries: response.libraries,
        builtins: response.builtins,
        tags: response.templatetags,
    })
}

/// Provenance of a template tag — either from a loadable library or a builtin
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TagProvenance {
    /// Tag requires `{% load X %}` to use
    Library {
        load_name: String,
        module: String,
    },
    /// Tag is always available (builtin)
    Builtin {
        module: String,
    },
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct TemplateTags {
    libraries: HashMap<String, String>,
    builtins: Vec<String>,
    tags: Vec<TemplateTag>,
}

impl TemplateTags {
    #[must_use]
    pub fn new(
        libraries: HashMap<String, String>,
        builtins: Vec<String>,
        tags: Vec<TemplateTag>,
    ) -> Self {
        Self {
            libraries,
            builtins,
            tags,
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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TemplateTag {
    name: String,
    provenance: TagProvenance,
    defining_module: String,
    doc: Option<String>,
}

impl TemplateTag {
    #[must_use]
    pub fn name(&self) -> &String {
        &self.name
    }

    #[must_use]
    pub fn provenance(&self) -> &TagProvenance {
        &self.provenance
    }

    /// The Python module where the tag function is defined (`tag_func.__module__`)
    #[must_use]
    pub fn defining_module(&self) -> &String {
        &self.defining_module
    }

    #[must_use]
    pub fn doc(&self) -> Option<&String> {
        self.doc.as_ref()
    }

    /// Returns the library load-name if this is a library tag, None for builtins.
    #[must_use]
    pub fn library_load_name(&self) -> Option<&str> {
        match &self.provenance {
            TagProvenance::Library { load_name, .. } => Some(load_name),
            TagProvenance::Builtin { .. } => None,
        }
    }

    /// Returns true if this tag is a builtin (always available without `{% load %}`)
    #[must_use]
    pub fn is_builtin(&self) -> bool {
        matches!(self.provenance, TagProvenance::Builtin { .. })
    }

    /// The Python module where this tag is registered (the library/builtin module).
    #[must_use]
    pub fn registration_module(&self) -> &str {
        match &self.provenance {
            TagProvenance::Library { module, .. } | TagProvenance::Builtin { module } => module,
        }
    }

    /// Create a library tag (for testing)
    #[must_use]
    pub fn new_library(name: &str, load_name: &str, module: &str, doc: Option<&str>) -> Self {
        Self {
            name: name.to_string(),
            provenance: TagProvenance::Library {
                load_name: load_name.to_string(),
                module: module.to_string(),
            },
            defining_module: module.to_string(),
            doc: doc.map(String::from),
        }
    }

    /// Create a builtin tag (for testing)
    #[must_use]
    pub fn new_builtin(name: &str, module: &str, doc: Option<&str>) -> Self {
        Self {
            name: name.to_string(),
            provenance: TagProvenance::Builtin {
                module: module.to_string(),
            },
            defining_module: module.to_string(),
            doc: doc.map(String::from),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_template_tag_library_provenance() {
        let tag = TemplateTag {
            name: "static".to_string(),
            provenance: TagProvenance::Library {
                load_name: "static".to_string(),
                module: "django.templatetags.static".to_string(),
            },
            defining_module: "django.templatetags.static".to_string(),
            doc: Some("Display static file URL".to_string()),
        };
        assert_eq!(tag.name(), "static");
        assert_eq!(tag.library_load_name(), Some("static"));
        assert!(!tag.is_builtin());
        assert_eq!(tag.registration_module(), "django.templatetags.static");
        assert_eq!(tag.defining_module(), "django.templatetags.static");
    }

    #[test]
    fn test_template_tag_builtin_provenance() {
        let tag = TemplateTag {
            name: "if".to_string(),
            provenance: TagProvenance::Builtin {
                module: "django.template.defaulttags".to_string(),
            },
            defining_module: "django.template.defaulttags".to_string(),
            doc: Some("Conditional block".to_string()),
        };
        assert_eq!(tag.name(), "if");
        assert_eq!(tag.library_load_name(), None);
        assert!(tag.is_builtin());
        assert_eq!(tag.registration_module(), "django.template.defaulttags");
    }

    #[test]
    fn test_template_tag_deserialization() {
        let json = r#"{
            "name": "trans",
            "provenance": {"library": {"load_name": "i18n", "module": "django.templatetags.i18n"}},
            "defining_module": "django.templatetags.i18n",
            "doc": "Translate text"
        }"#;
        let tag: TemplateTag = serde_json::from_str(json).expect("Should deserialize");
        assert_eq!(tag.name(), "trans");
        assert_eq!(tag.library_load_name(), Some("i18n"));
        assert_eq!(tag.defining_module(), "django.templatetags.i18n");
    }

    #[test]
    fn test_template_tag_builtin_deserialization() {
        let json = r#"{
            "name": "if",
            "provenance": {"builtin": {"module": "django.template.defaulttags"}},
            "defining_module": "django.template.defaulttags",
            "doc": null
        }"#;
        let tag: TemplateTag = serde_json::from_str(json).expect("Should deserialize");
        assert_eq!(tag.name(), "if");
        assert!(tag.is_builtin());
        assert_eq!(tag.library_load_name(), None);
    }

    #[test]
    fn test_template_tags_registry_data() {
        let mut libraries = HashMap::new();
        libraries.insert(
            "static".to_string(),
            "django.templatetags.static".to_string(),
        );
        libraries.insert(
            "i18n".to_string(),
            "django.templatetags.i18n".to_string(),
        );

        let tags = TemplateTags {
            libraries,
            builtins: vec![
                "django.template.defaulttags".to_string(),
                "django.template.defaultfilters".to_string(),
            ],
            tags: vec![
                TemplateTag::new_builtin("if", "django.template.defaulttags", None),
                TemplateTag::new_library(
                    "static",
                    "static",
                    "django.templatetags.static",
                    None,
                ),
            ],
        };

        assert_eq!(tags.len(), 2);
        assert!(!tags.is_empty());
        assert_eq!(tags.libraries().len(), 2);
        assert_eq!(tags.builtins().len(), 2);
        assert!(tags.iter().next().unwrap().is_builtin());
    }

    #[test]
    fn test_template_tag_constructors() {
        let lib_tag = TemplateTag::new_library("static", "static", "django.templatetags.static", Some("doc"));
        assert_eq!(lib_tag.name(), "static");
        assert_eq!(lib_tag.library_load_name(), Some("static"));
        assert!(!lib_tag.is_builtin());
        assert_eq!(lib_tag.doc(), Some(&"doc".to_string()));

        let builtin_tag = TemplateTag::new_builtin("if", "django.template.defaulttags", None);
        assert_eq!(builtin_tag.name(), "if");
        assert!(builtin_tag.is_builtin());
        assert_eq!(builtin_tag.doc(), None);
    }
}

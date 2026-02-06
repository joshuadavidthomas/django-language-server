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
pub struct TemplatetagsRequest;

#[derive(Deserialize)]
pub struct TemplatetagsResponse {
    pub libraries: HashMap<String, String>,
    pub builtins: Vec<String>,
    pub templatetags: Vec<TemplateTag>,
}

impl InspectorRequest for TemplatetagsRequest {
    const NAME: &'static str = "templatetags";
    type Response = TemplatetagsResponse;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateInventoryRequest;

#[derive(Deserialize)]
pub struct TemplateInventoryResponse {
    pub libraries: HashMap<String, String>,
    pub builtins: Vec<String>,
    pub templatetags: Vec<TemplateTag>,
    pub templatefilters: Vec<TemplateFilter>,
}

impl InspectorRequest for TemplateInventoryRequest {
    const NAME: &'static str = "template_inventory";
    type Response = TemplateInventoryResponse;
}

/// Request for Python environment info (including `sys.path`).
#[derive(Debug, Clone, Serialize)]
pub struct PythonEnvRequest;

/// Response from the `python_env` query.
#[derive(Debug, Clone, Deserialize)]
pub struct PythonEnvResponse {
    pub sys_path: Vec<String>,
}

impl InspectorRequest for PythonEnvRequest {
    const NAME: &'static str = "python_env";
    type Response = PythonEnvResponse;
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

/// Provenance of a template filter — either from a loadable library or a builtin
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FilterProvenance {
    /// Filter requires `{% load X %}` to use
    Library { load_name: String, module: String },
    /// Filter is always available (builtin)
    Builtin { module: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TemplateFilter {
    name: String,
    provenance: FilterProvenance,
    defining_module: String,
    doc: Option<String>,
}

impl TemplateFilter {
    /// Create a library filter (for testing)
    #[must_use]
    pub fn new_library(name: &str, load_name: &str, module: &str, doc: Option<&str>) -> Self {
        Self {
            name: name.to_string(),
            provenance: FilterProvenance::Library {
                load_name: load_name.to_string(),
                module: module.to_string(),
            },
            defining_module: module.to_string(),
            doc: doc.map(String::from),
        }
    }

    /// Create a builtin filter (for testing)
    #[must_use]
    pub fn new_builtin(name: &str, module: &str, doc: Option<&str>) -> Self {
        Self {
            name: name.to_string(),
            provenance: FilterProvenance::Builtin {
                module: module.to_string(),
            },
            defining_module: module.to_string(),
            doc: doc.map(String::from),
        }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn provenance(&self) -> &FilterProvenance {
        &self.provenance
    }

    /// The Python module where the filter function is defined (`filter_func.__module__`)
    #[must_use]
    pub fn defining_module(&self) -> &str {
        &self.defining_module
    }

    #[must_use]
    pub fn doc(&self) -> Option<&str> {
        self.doc.as_deref()
    }

    /// Returns the library load-name if this is a library filter, None for builtins.
    #[must_use]
    pub fn library_load_name(&self) -> Option<&str> {
        match &self.provenance {
            FilterProvenance::Library { load_name, .. } => Some(load_name),
            FilterProvenance::Builtin { .. } => None,
        }
    }

    /// Returns true if this filter is a builtin (always available without `{% load %}`)
    #[must_use]
    pub fn is_builtin(&self) -> bool {
        matches!(self.provenance, FilterProvenance::Builtin { .. })
    }

    /// The Python module where this filter is registered (the library/builtin module).
    #[must_use]
    pub fn registration_module(&self) -> &str {
        match &self.provenance {
            FilterProvenance::Library { module, .. } | FilterProvenance::Builtin { module } => {
                module
            }
        }
    }
}

/// Combined inspector inventory (tags + filters) stored on Project.
///
/// This is a single snapshot to prevent split-brain between tag and filter data.
/// Per M2 architecture, this is stored as a Project field (Salsa input), not
/// computed by a tracked query calling the inspector.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct InspectorInventory {
    libraries: HashMap<String, String>,
    builtins: Vec<String>,
    tags: Vec<TemplateTag>,
    filters: Vec<TemplateFilter>,
}

impl InspectorInventory {
    #[must_use]
    pub fn new(
        libraries: HashMap<String, String>,
        builtins: Vec<String>,
        tags: Vec<TemplateTag>,
        filters: Vec<TemplateFilter>,
    ) -> Self {
        Self {
            libraries,
            builtins,
            tags,
            filters,
        }
    }

    #[must_use]
    pub fn tags(&self) -> &[TemplateTag] {
        &self.tags
    }

    #[must_use]
    pub fn filters(&self) -> &[TemplateFilter] {
        &self.filters
    }

    #[must_use]
    pub fn libraries(&self) -> &HashMap<String, String> {
        &self.libraries
    }

    #[must_use]
    pub fn builtins(&self) -> &[String] {
        &self.builtins
    }

    #[must_use]
    pub fn tag_count(&self) -> usize {
        self.tags.len()
    }

    #[must_use]
    pub fn filter_count(&self) -> usize {
        self.filters.len()
    }

    pub fn iter_tags(&self) -> impl Iterator<Item = &TemplateTag> {
        self.tags.iter()
    }

    pub fn iter_filters(&self) -> impl Iterator<Item = &TemplateFilter> {
        self.filters.iter()
    }
}

/// Provenance of a template tag — either from a loadable library or a builtin
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TagProvenance {
    /// Tag requires `{% load X %}` to use
    Library { load_name: String, module: String },
    /// Tag is always available (builtin)
    Builtin { module: String },
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
        libraries.insert("i18n".to_string(), "django.templatetags.i18n".to_string());

        let tags = TemplateTags {
            libraries,
            builtins: vec![
                "django.template.defaulttags".to_string(),
                "django.template.defaultfilters".to_string(),
            ],
            tags: vec![
                TemplateTag::new_builtin("if", "django.template.defaulttags", None),
                TemplateTag::new_library("static", "static", "django.templatetags.static", None),
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
        let lib_tag = TemplateTag::new_library(
            "static",
            "static",
            "django.templatetags.static",
            Some("doc"),
        );
        assert_eq!(lib_tag.name(), "static");
        assert_eq!(lib_tag.library_load_name(), Some("static"));
        assert!(!lib_tag.is_builtin());
        assert_eq!(lib_tag.doc(), Some(&"doc".to_string()));

        let builtin_tag = TemplateTag::new_builtin("if", "django.template.defaulttags", None);
        assert_eq!(builtin_tag.name(), "if");
        assert!(builtin_tag.is_builtin());
        assert_eq!(builtin_tag.doc(), None);
    }

    #[test]
    fn test_template_filter_library_provenance() {
        let filter = TemplateFilter::new_library(
            "date",
            "humanize",
            "django.contrib.humanize.templatetags.humanize",
            Some("Format a date"),
        );
        assert_eq!(filter.name(), "date");
        assert_eq!(filter.library_load_name(), Some("humanize"));
        assert!(!filter.is_builtin());
        assert_eq!(
            filter.registration_module(),
            "django.contrib.humanize.templatetags.humanize"
        );
        assert_eq!(
            filter.defining_module(),
            "django.contrib.humanize.templatetags.humanize"
        );
        assert_eq!(filter.doc(), Some("Format a date"));
    }

    #[test]
    fn test_template_filter_builtin_provenance() {
        let filter = TemplateFilter::new_builtin("title", "django.template.defaultfilters", None);
        assert_eq!(filter.name(), "title");
        assert!(filter.is_builtin());
        assert_eq!(filter.library_load_name(), None);
        assert_eq!(
            filter.registration_module(),
            "django.template.defaultfilters"
        );
        assert_eq!(filter.doc(), None);
    }

    #[test]
    fn test_template_filter_deserialization() {
        let json = r#"{
            "name": "date",
            "provenance": {"library": {"load_name": "humanize", "module": "django.contrib.humanize.templatetags.humanize"}},
            "defining_module": "django.contrib.humanize.templatetags.humanize",
            "doc": "Format a date"
        }"#;
        let filter: TemplateFilter = serde_json::from_str(json).expect("Should deserialize");
        assert_eq!(filter.name(), "date");
        assert_eq!(filter.library_load_name(), Some("humanize"));
        assert_eq!(filter.doc(), Some("Format a date"));
    }

    #[test]
    fn test_template_filter_builtin_deserialization() {
        let json = r#"{
            "name": "title",
            "provenance": {"builtin": {"module": "django.template.defaultfilters"}},
            "defining_module": "django.template.defaultfilters",
            "doc": null
        }"#;
        let filter: TemplateFilter = serde_json::from_str(json).expect("Should deserialize");
        assert_eq!(filter.name(), "title");
        assert!(filter.is_builtin());
        assert_eq!(filter.library_load_name(), None);
    }

    #[test]
    fn test_inspector_inventory() {
        let mut libraries = HashMap::new();
        libraries.insert("i18n".to_string(), "django.templatetags.i18n".to_string());

        let inv = InspectorInventory::new(
            libraries,
            vec!["django.template.defaulttags".to_string()],
            vec![TemplateTag::new_builtin(
                "if",
                "django.template.defaulttags",
                None,
            )],
            vec![
                TemplateFilter::new_builtin("title", "django.template.defaultfilters", None),
                TemplateFilter::new_library("localize", "l10n", "django.templatetags.l10n", None),
            ],
        );

        assert_eq!(inv.tag_count(), 1);
        assert_eq!(inv.filter_count(), 2);
        assert_eq!(inv.libraries().len(), 1);
        assert_eq!(inv.builtins().len(), 1);
        assert!(inv.iter_tags().next().unwrap().is_builtin());
        assert!(inv.iter_filters().next().unwrap().is_builtin());
    }

    #[test]
    fn test_template_inventory_response_deserialization() {
        let json = r#"{
            "libraries": {"i18n": "django.templatetags.i18n"},
            "builtins": ["django.template.defaulttags"],
            "templatetags": [
                {
                    "name": "if",
                    "provenance": {"builtin": {"module": "django.template.defaulttags"}},
                    "defining_module": "django.template.defaulttags",
                    "doc": null
                }
            ],
            "templatefilters": [
                {
                    "name": "title",
                    "provenance": {"builtin": {"module": "django.template.defaultfilters"}},
                    "defining_module": "django.template.defaultfilters",
                    "doc": "Convert to titlecase"
                }
            ]
        }"#;
        let resp: TemplateInventoryResponse =
            serde_json::from_str(json).expect("Should deserialize");
        assert_eq!(resp.libraries.len(), 1);
        assert_eq!(resp.builtins.len(), 1);
        assert_eq!(resp.templatetags.len(), 1);
        assert_eq!(resp.templatefilters.len(), 1);
        assert_eq!(resp.templatetags[0].name(), "if");
        assert_eq!(resp.templatefilters[0].name(), "title");
    }
}

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
    /// Load-name → module path mapping (from engine.libraries)
    libraries: HashMap<String, String>,
    /// Ordered builtin module paths (from engine.builtins)
    builtins: Vec<String>,
    /// Tag inventory
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
    Some(TemplateTags::new(
        response.libraries,
        response.builtins,
        response.templatetags,
    ))
}

/// Provenance of a template tag - either from a loadable library or a builtin
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TagProvenance {
    /// Tag requires `{% load X %}` to use
    Library {
        /// The name used in `{% load X %}` (e.g., "static", "i18n")
        load_name: String,
        /// The Python module path where the library is registered
        module: String,
    },
    /// Tag is always available (builtin)
    Builtin {
        /// The Python module path where the builtin is registered
        module: String,
    },
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct TemplateTags {
    /// Load-name → module path mapping (from engine.libraries)
    libraries: HashMap<String, String>,
    /// Ordered builtin module paths (from engine.builtins)
    builtins: Vec<String>,
    /// Tag inventory
    tags: Vec<TemplateTag>,
}

impl TemplateTags {
    /// Create a new `TemplateTags` (primarily for testing)
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

    /// Construct from inspector response data (M1 payload shape).
    #[must_use]
    pub fn from_response(
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

    /// Get the tag list
    #[must_use]
    pub fn tags(&self) -> &[TemplateTag] {
        &self.tags
    }

    /// Get the libraries mapping (`load_name` → `module_path`)
    #[must_use]
    pub fn libraries(&self) -> &HashMap<String, String> {
        &self.libraries
    }

    /// Get the ordered builtin module paths
    #[must_use]
    pub fn builtins(&self) -> &[String] {
        &self.builtins
    }

    /// Iterate over tags (convenience method)
    pub fn iter(&self) -> impl Iterator<Item = &TemplateTag> {
        self.tags.iter()
    }

    /// Number of tags
    #[must_use]
    pub fn len(&self) -> usize {
        self.tags.len()
    }

    /// Check if empty
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
    /// This is where the actual code lives, useful for docs/jump-to-def.
    #[must_use]
    pub fn defining_module(&self) -> &String {
        &self.defining_module
    }

    #[must_use]
    pub fn doc(&self) -> Option<&String> {
        self.doc.as_ref()
    }

    /// Returns the library load-name if this is a library tag, None for builtins.
    /// This is the name used in `{% load X %}`.
    #[must_use]
    pub fn library_load_name(&self) -> Option<&str> {
        match &self.provenance {
            TagProvenance::Library { load_name, .. } => Some(load_name),
            TagProvenance::Builtin { .. } => None,
        }
    }

    /// Returns true if this tag is a builtin (always available without {% load %})
    #[must_use]
    pub fn is_builtin(&self) -> bool {
        matches!(&self.provenance, TagProvenance::Builtin { .. })
    }

    /// The Python module where this tag is registered (the library/builtin module).
    /// For libraries, this is the module in engine.libraries.
    /// For builtins, this is the module in engine.builtins.
    #[must_use]
    pub fn registration_module(&self) -> &str {
        match &self.provenance {
            TagProvenance::Library { module, .. } | TagProvenance::Builtin { module } => module,
        }
    }
}

impl TemplateTag {
    /// Create a library tag (for testing)
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

/// Provenance of a template filter - either from a loadable library or a builtin
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FilterProvenance {
    /// Filter requires `{% load X %}` to use
    Library {
        load_name: String,
        module: String,
    },
    /// Filter is always available (builtin)
    Builtin {
        module: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TemplateFilter {
    name: String,
    provenance: FilterProvenance,
    defining_module: String,
    doc: Option<String>,
}

impl TemplateFilter {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn provenance(&self) -> &FilterProvenance {
        &self.provenance
    }

    /// The Python module where the filter function is defined (`filter_func.__module__`)
    /// This is where the actual code lives, useful for docs/jump-to-def.
    #[must_use]
    pub fn defining_module(&self) -> &str {
        &self.defining_module
    }

    #[must_use]
    pub fn doc(&self) -> Option<&str> {
        self.doc.as_deref()
    }

    /// Returns the library load-name if this is a library filter, None for builtins.
    /// This is the name used in `{% load X %}`.
    #[must_use]
    pub fn library_load_name(&self) -> Option<&str> {
        match &self.provenance {
            FilterProvenance::Library { load_name, .. } => Some(load_name),
            FilterProvenance::Builtin { .. } => None,
        }
    }

    /// Returns true if this filter is a builtin (always available without {% load %})
    #[must_use]
    pub fn is_builtin(&self) -> bool {
        matches!(&self.provenance, FilterProvenance::Builtin { .. })
    }

    /// The Python module where this filter is registered (the library/builtin module).
    ///
    /// This is the **registration module** (where `@register.filter` happened), and is the
    /// correct module to use for AST-based rule extraction (M5) and for collision-safe arity
    /// lookup (M6, keyed by `{registration_module, name}`).
    ///
    /// Do **not** confuse this with `defining_module()` (where the function is defined).
    #[must_use]
    pub fn registration_module(&self) -> &str {
        match &self.provenance {
            FilterProvenance::Library { module, .. } | FilterProvenance::Builtin { module } => {
                module
            }
        }
    }
}

impl TemplateFilter {
    /// Create a library filter (for testing)
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
}

/// Combined inspector inventory (tags + filters) stored on Project.
///
/// This is a single snapshot to prevent split-brain between tag and filter data.
/// Per M2 architecture, this is stored as a Project field (Salsa input), not
/// computed by a tracked query calling the inspector.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct InspectorInventory {
    /// Load-name → module path mapping
    libraries: HashMap<String, String>,
    /// Ordered builtin module paths
    builtins: Vec<String>,
    /// Tag inventory
    tags: Vec<TemplateTag>,
    /// Filter inventory
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

    /// Construct from inspector response data.
    #[must_use]
    pub fn from_response(response: TemplateInventoryResponse) -> Self {
        Self {
            libraries: response.libraries,
            builtins: response.builtins,
            tags: response.templatetags,
            filters: response.templatefilters,
        }
    }

    /// Get the tag list
    #[must_use]
    pub fn tags(&self) -> &[TemplateTag] {
        &self.tags
    }

    /// Get the filter list
    #[must_use]
    pub fn filters(&self) -> &[TemplateFilter] {
        &self.filters
    }

    /// Get the libraries mapping (`load_name` → `module_path`)
    #[must_use]
    pub fn libraries(&self) -> &HashMap<String, String> {
        &self.libraries
    }

    /// Get the ordered builtin module paths
    #[must_use]
    pub fn builtins(&self) -> &[String] {
        &self.builtins
    }

    /// Number of tags
    #[must_use]
    pub fn tag_count(&self) -> usize {
        self.tags.len()
    }

    /// Number of filters
    #[must_use]
    pub fn filter_count(&self) -> usize {
        self.filters.len()
    }
}

#[derive(Serialize)]
pub struct TemplateInventoryRequest;

#[derive(Deserialize)]
pub struct TemplateInventoryResponse {
    /// Load-name → module path mapping (from engine.libraries)
    pub libraries: HashMap<String, String>,
    /// Ordered builtin module paths (from engine.builtins)
    pub builtins: Vec<String>,
    /// Tag inventory
    pub templatetags: Vec<TemplateTag>,
    /// Filter inventory
    pub templatefilters: Vec<TemplateFilter>,
}

impl InspectorRequest for TemplateInventoryRequest {
    const NAME: &'static str = "template_inventory";
    type Response = TemplateInventoryResponse;
}

/// Request for Python environment information (sys.path, etc.)
#[derive(Serialize)]
pub struct PythonEnvRequest;

/// Response containing Python environment details
#[derive(Deserialize)]
pub struct PythonEnvResponse {
    pub sys_base_prefix: Utf8PathBuf,
    pub sys_executable: Utf8PathBuf,
    pub sys_path: Vec<Utf8PathBuf>,
    pub sys_platform: String,
    pub sys_prefix: Utf8PathBuf,
    pub sys_version_info: (u8, u8, u8, String, u8),
}

impl InspectorRequest for PythonEnvRequest {
    const NAME: &'static str = "python_env";
    type Response = PythonEnvResponse;
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
    }

    #[test]
    fn test_template_tags_registry_data() {
        let mut libraries = HashMap::new();
        libraries.insert("static".to_string(), "django.templatetags.static".to_string());
        libraries.insert("i18n".to_string(), "django.templatetags.i18n".to_string());

        let tags = TemplateTags {
            libraries,
            builtins: vec![
                "django.template.defaulttags".to_string(),
                "django.template.defaultfilters".to_string(),
            ],
            tags: vec![
                TemplateTag {
                    name: "if".to_string(),
                    provenance: TagProvenance::Builtin {
                        module: "django.template.defaulttags".to_string(),
                    },
                    defining_module: "django.template.defaulttags".to_string(),
                    doc: None,
                },
                TemplateTag {
                    name: "static".to_string(),
                    provenance: TagProvenance::Library {
                        load_name: "static".to_string(),
                        module: "django.templatetags.static".to_string(),
                    },
                    defining_module: "django.templatetags.static".to_string(),
                    doc: None,
                },
            ],
        };

        assert_eq!(tags.len(), 2);
        assert_eq!(tags.libraries().len(), 2);
        assert_eq!(tags.builtins().len(), 2);
        assert!(tags.iter().next().unwrap().is_builtin());
    }

    #[test]
    fn test_template_tag_constructors() {
        let library_tag = TemplateTag::new_library(
            "static",
            "static",
            "django.templatetags.static",
            Some("Static tag"),
        );
        assert_eq!(library_tag.name(), "static");
        assert_eq!(library_tag.library_load_name(), Some("static"));
        assert!(!library_tag.is_builtin());

        let builtin_tag =
            TemplateTag::new_builtin("if", "django.template.defaulttags", Some("If tag"));
        assert_eq!(builtin_tag.name(), "if");
        assert_eq!(builtin_tag.library_load_name(), None);
        assert!(builtin_tag.is_builtin());
    }
}

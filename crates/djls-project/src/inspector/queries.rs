use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

#[derive(Serialize, Deserialize)]
#[serde(tag = "query", content = "args")]
#[serde(rename_all = "snake_case")]
pub enum Query {
    PythonEnv,
    Templatetags,
    DjangoInit,
}

/// Enum representing different kinds of inspector queries for Salsa tracking
#[derive(Clone, Debug, PartialEq, Eq, Hash, Copy)]
pub enum InspectorQueryKind {
    TemplateTags,
    DjangoAvailable,
    SettingsModule,
}

#[derive(Serialize, Deserialize)]
pub struct PythonEnvironmentQueryData {
    pub sys_base_prefix: PathBuf,
    pub sys_executable: PathBuf,
    pub sys_path: Vec<PathBuf>,
    pub sys_platform: String,
    pub sys_prefix: PathBuf,
    pub sys_version_info: (u32, u32, u32, VersionReleaseLevel, u32),
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VersionReleaseLevel {
    Alpha,
    Beta,
    Candidate,
    Final,
}

#[derive(Serialize, Deserialize)]
pub struct TemplateTagQueryData {
    pub templatetags: Vec<TemplateTag>,
}

#[derive(Serialize, Deserialize)]
pub struct TemplateTag {
    pub name: String,
    pub module: String,
    pub doc: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inspector_query_kind_enum() {
        // Test that InspectorQueryKind variants exist and are copyable
        let template_tags = InspectorQueryKind::TemplateTags;
        let django_available = InspectorQueryKind::DjangoAvailable;
        let settings_module = InspectorQueryKind::SettingsModule;

        // Test that they can be copied
        assert_eq!(template_tags, InspectorQueryKind::TemplateTags);
        assert_eq!(django_available, InspectorQueryKind::DjangoAvailable);
        assert_eq!(settings_module, InspectorQueryKind::SettingsModule);
    }
}

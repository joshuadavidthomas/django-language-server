use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash, Copy)]
#[serde(tag = "query", content = "args")]
#[serde(rename_all = "snake_case")]
pub enum Query {
    DjangoInit,
    PythonEnv,
    Templatetags,
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
    fn test_query_enum() {
        // Test that Query variants exist and are copyable
        let python_env = Query::PythonEnv;
        let templatetags = Query::Templatetags;
        let django_init = Query::DjangoInit;

        // Test that they can be copied
        assert_eq!(python_env, Query::PythonEnv);
        assert_eq!(templatetags, Query::Templatetags);
        assert_eq!(django_init, Query::DjangoInit);
    }
}

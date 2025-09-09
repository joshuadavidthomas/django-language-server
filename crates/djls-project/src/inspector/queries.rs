use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(tag = "query", content = "args")]
#[serde(rename_all = "snake_case")]
pub enum Query {
    PythonEnv,
    Templatetags,
    DjangoInit,
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
pub struct PythonEnvironmentQueryData {
    pub sys_base_prefix: PathBuf,
    pub sys_executable: PathBuf,
    pub sys_path: Vec<PathBuf>,
    pub sys_platform: String,
    pub sys_prefix: PathBuf,
    pub sys_version_info: (u32, u32, u32, VersionReleaseLevel, u32),
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

#[derive(Serialize, Deserialize)]
pub struct DjangoInitQueryData {
    pub success: bool,
    pub message: Option<String>,
}

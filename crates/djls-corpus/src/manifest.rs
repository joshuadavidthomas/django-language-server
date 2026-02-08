use camino::Utf8Path;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub corpus: CorpusConfig,
    #[serde(default, rename = "package")]
    pub packages: Vec<Package>,
    #[serde(default, rename = "repo")]
    pub repos: Vec<Repo>,
}

#[derive(Debug, Deserialize)]
pub struct CorpusConfig {
    pub root_dir: String,
}

#[derive(Debug, Deserialize)]
pub struct Package {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Deserialize)]
pub struct Repo {
    pub name: String,
    pub url: String,
    #[serde(rename = "ref")]
    pub git_ref: String,
}

impl Manifest {
    pub fn load(path: &Utf8Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path.as_std_path())?;
        let manifest: Self = toml::from_str(&content)?;
        Ok(manifest)
    }
}

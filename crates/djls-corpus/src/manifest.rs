use camino::Utf8Component;
use camino::Utf8Path;
use camino::Utf8PathBuf;
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
    pub sha256: String,
}

#[derive(Debug, Deserialize)]
pub struct Repo {
    pub name: String,
    pub url: String,
    #[serde(rename = "ref")]
    pub git_ref: String,
    pub sha256: String,
}

impl Manifest {
    pub fn load(path: &Utf8Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path.as_std_path())?;
        let manifest: Self = toml::from_str(&content)?;
        Ok(manifest)
    }

    /// Resolve and validate the corpus root directory.
    ///
    /// Rejects absolute paths and paths containing `..` components to
    /// prevent `remove_dir_all` or file writes outside the base directory.
    pub fn corpus_root(&self, base_dir: &Utf8Path) -> anyhow::Result<Utf8PathBuf> {
        let root_dir = &self.corpus.root_dir;
        let root_path = Utf8Path::new(root_dir);

        anyhow::ensure!(
            !root_path.as_std_path().is_absolute(),
            "corpus root_dir must be a relative path, got: {root_dir}"
        );

        anyhow::ensure!(
            !root_path
                .components()
                .any(|c| matches!(c, Utf8Component::ParentDir)),
            "corpus root_dir must not contain '..' components, got: {root_dir}"
        );

        Ok(base_dir.join(root_dir))
    }
}

use camino::Utf8Component;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use serde::Deserialize;
use serde::Deserializer;

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
    #[serde(deserialize_with = "deserialize_pypi_name")]
    pub name: String,
    pub version: String,
}

/// Deserialize and normalize a `PyPI` package name per PEP 503: lowercase,
/// runs of `[-_.]` become a single `-`.
pub(crate) fn deserialize_pypi_name<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let raw = String::deserialize(deserializer)?;
    let mut result = String::with_capacity(raw.len());
    let mut prev_sep = false;
    for c in raw.chars() {
        if c == '-' || c == '_' || c == '.' {
            if !prev_sep && !result.is_empty() {
                result.push('-');
            }
            prev_sep = true;
        } else {
            result.push(c.to_ascii_lowercase());
            prev_sep = false;
        }
    }
    if result.ends_with('-') {
        result.pop();
    }
    Ok(result)
}

#[derive(Debug, Deserialize)]
pub struct Repo {
    pub name: String,
    pub url: String,
    /// Optional ref to track: a branch (`main`), tag (`v1.0.0`), or SHA.
    /// When omitted, `lock` resolves to the latest tag.
    #[serde(rename = "ref")]
    pub git_ref: Option<String>,
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

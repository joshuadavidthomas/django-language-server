use camino::Utf8Path;
use camino::Utf8PathBuf;
use serde::Deserialize;

#[cfg(test)]
const MANIFEST_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/manifest.toml");
#[cfg(test)]
const CRATE_DIR: &str = env!("CARGO_MANIFEST_DIR");

#[derive(Debug, Deserialize)]
pub struct Manifest {
    corpus: CorpusConfig,
    #[serde(default, rename = "repo")]
    pub(crate) repos: Vec<Repo>,
    #[cfg(test)]
    #[serde(default, rename = "fixture")]
    fixtures: Vec<Fixture>,
}

#[derive(Debug, Deserialize)]
pub struct CorpusConfig {
    pub root_dir: String,
}

#[derive(Debug, Deserialize)]
pub struct Repo {
    pub(crate) name: String,
    pub(crate) url: String,
    /// Optional ref to track: a branch (`main`), tag (`v1.0.0`), or SHA.
    /// When omitted, `lock` resolves to the latest tag.
    #[serde(rename = "ref")]
    pub(crate) git_ref: Option<String>,
    #[cfg(test)]
    #[serde(default)]
    django_settings_module: Option<String>,
    #[cfg(test)]
    #[serde(default)]
    django_settings_modules: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct Fixture {
    #[cfg(test)]
    name: String,
    #[cfg(test)]
    path: String,
    #[cfg(test)]
    #[serde(default)]
    django_settings_module: Option<String>,
    #[cfg(test)]
    #[serde(default)]
    django_settings_modules: Vec<String>,
}

impl Manifest {
    #[cfg(test)]
    fn load_default() -> anyhow::Result<Self> {
        Self::load(Utf8Path::new(MANIFEST_PATH))
    }

    pub fn load(path: &Utf8Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path.as_std_path())?;
        Ok(toml::from_str(&content)?)
    }

    #[must_use]
    pub fn corpus_root(&self, base_dir: &Utf8Path) -> Utf8PathBuf {
        base_dir.join(&self.corpus.root_dir)
    }
}

impl Repo {
    #[cfg(test)]
    fn django_settings_modules(&self) -> impl Iterator<Item = &str> {
        self.django_settings_module
            .iter()
            .map(String::as_str)
            .chain(self.django_settings_modules.iter().map(String::as_str))
    }
}

#[cfg(test)]
impl Fixture {
    fn django_settings_modules(&self) -> impl Iterator<Item = &str> {
        self.django_settings_module
            .iter()
            .map(String::as_str)
            .chain(self.django_settings_modules.iter().map(String::as_str))
    }

    #[must_use]
    fn root_path(&self) -> Utf8PathBuf {
        Utf8Path::new(CRATE_DIR).join(&self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_loads_django_project_selectors() {
        let manifest = Manifest::load_default().unwrap();

        for (repo, settings) in [
            ("archivebox", &["archivebox.core.settings"][..]),
            ("healthchecks", &["hc.settings"]),
            ("inventree", &["InvenTree.settings"]),
            ("netbox", &["netbox.settings"]),
            ("pretix", &["pretix.settings"]),
            ("sentry", &["sentry.conf.server"]),
            ("django-allauth", &["tests.projects.account_only.settings"]),
        ] {
            let repo = manifest
                .repos
                .iter()
                .find(|candidate| candidate.name == repo)
                .unwrap_or_else(|| panic!("missing repo `{repo}`"));
            assert_eq!(
                repo.django_settings_modules()
                    .collect::<Vec<_>>()
                    .as_slice(),
                settings
            );
        }

        let fixture = manifest
            .fixtures
            .iter()
            .find(|candidate| candidate.name == "gh401-multisite")
            .expect("missing GH-401 fixture");
        assert_eq!(
            fixture
                .django_settings_modules()
                .collect::<Vec<_>>()
                .as_slice(),
            ["site1.settings.dev", "site2.settings.dev"]
        );
    }

    #[test]
    fn django_fixture_roots_exist() {
        let manifest = Manifest::load_default().unwrap();

        for fixture in manifest
            .fixtures
            .iter()
            .filter(|fixture| fixture.django_settings_modules().next().is_some())
        {
            let root = fixture.root_path();
            assert!(
                root.as_std_path().is_dir(),
                "fixture `{}` root `{root}` does not exist",
                fixture.name
            );
        }
    }
}

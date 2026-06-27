use anyhow::ensure;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Manifest {
    corpus: CorpusConfig,
    #[serde(default, rename = "repo")]
    pub(crate) repos: Vec<Repo>,
    #[serde(default, rename = "fixture")]
    fixtures: Vec<Fixture>,
}

#[derive(Debug, Deserialize)]
struct CorpusConfig {
    root_dir: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Repo {
    pub(crate) name: String,
    pub(crate) url: String,
    /// Optional ref to track: a branch (`main`), tag (`v1.0.0`), or SHA.
    /// When omitted, `lock` resolves to the latest tag.
    #[serde(rename = "ref")]
    pub(crate) git_ref: Option<String>,
    #[serde(default)]
    django_settings_module: Option<String>,
    #[serde(default)]
    django_settings_modules: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Fixture {
    name: String,
    path: String,
    #[serde(default)]
    django_settings_module: Option<String>,
    #[serde(default)]
    django_settings_modules: Vec<String>,
}

impl Manifest {
    pub fn load(path: &Utf8Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path.as_std_path())?;
        let manifest = toml::from_str::<Self>(&content)?;
        let manifest_dir = path.parent().unwrap_or_else(|| Utf8Path::new("."));
        manifest.validate(manifest_dir)?;
        Ok(manifest)
    }

    #[must_use]
    pub fn corpus_root(&self, base_dir: &Utf8Path) -> Utf8PathBuf {
        base_dir.join(&self.corpus.root_dir)
    }

    fn validate(&self, manifest_dir: &Utf8Path) -> anyhow::Result<()> {
        for repo in &self.repos {
            repo.validate()?;
        }
        for fixture in &self.fixtures {
            fixture.validate(manifest_dir)?;
        }
        Ok(())
    }
}

impl Repo {
    fn django_settings_modules(&self) -> impl Iterator<Item = &str> {
        self.django_settings_module
            .iter()
            .map(String::as_str)
            .chain(self.django_settings_modules.iter().map(String::as_str))
    }

    fn validate(&self) -> anyhow::Result<()> {
        for module in self.django_settings_modules() {
            ensure!(
                !module.trim().is_empty(),
                "repo `{}` has an empty Django settings module selector",
                self.name
            );
        }
        Ok(())
    }
}

impl Fixture {
    #[must_use]
    fn root_path(&self, manifest_dir: &Utf8Path) -> Utf8PathBuf {
        manifest_dir.join(&self.path)
    }

    fn django_settings_modules(&self) -> impl Iterator<Item = &str> {
        self.django_settings_module
            .iter()
            .map(String::as_str)
            .chain(self.django_settings_modules.iter().map(String::as_str))
    }

    fn validate(&self, manifest_dir: &Utf8Path) -> anyhow::Result<()> {
        ensure!(!self.name.trim().is_empty(), "fixture name cannot be empty");
        ensure!(
            !self.path.trim().is_empty(),
            "fixture `{}` path cannot be empty",
            self.name
        );

        let root = self.root_path(manifest_dir);
        ensure!(
            root.as_std_path().is_dir(),
            "fixture `{}` root `{root}` does not exist",
            self.name
        );

        for module in self.django_settings_modules() {
            ensure!(
                !module.trim().is_empty(),
                "fixture `{}` has an empty Django settings module selector",
                self.name
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_default_manifest() -> Manifest {
        let path = Utf8Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/manifest.toml"));
        Manifest::load(path).unwrap()
    }

    #[test]
    fn manifest_loads_django_project_selectors() {
        let manifest = load_default_manifest();

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
        let manifest = load_default_manifest();
        let manifest_dir = Utf8Path::new(env!("CARGO_MANIFEST_DIR"));

        for fixture in manifest
            .fixtures
            .iter()
            .filter(|fixture| fixture.django_settings_modules().next().is_some())
        {
            let root = fixture.root_path(manifest_dir);
            assert!(
                root.as_std_path().is_dir(),
                "fixture `{}` root `{root}` does not exist",
                fixture.name
            );
        }
    }
}

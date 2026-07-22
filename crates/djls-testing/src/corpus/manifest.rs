use anyhow::ensure;
use camino::Utf8Component;
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
    #[serde(default)]
    project_root: Option<Utf8PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepoSettingsProject<'a> {
    pub(crate) repo_name: &'a str,
    pub(crate) repo_url: &'a str,
    pub(crate) repo_ref: Option<&'a str>,
    pub(crate) relative_root: Option<&'a Utf8Path>,
    pub(crate) django_settings_modules: Vec<&'a str>,
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

    #[must_use]
    pub(crate) fn repo_settings_projects(&self) -> Vec<RepoSettingsProject<'_>> {
        self.repos
            .iter()
            .filter_map(|repo| {
                let django_settings_modules: Vec<_> = repo.django_settings_modules().collect();
                (!django_settings_modules.is_empty()).then_some(RepoSettingsProject {
                    repo_name: repo.name.as_str(),
                    repo_url: repo.url.as_str(),
                    repo_ref: repo.git_ref.as_deref(),
                    relative_root: repo.project_root.as_deref(),
                    django_settings_modules,
                })
            })
            .collect()
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
        let mut name_components = Utf8Path::new(&self.name).components();
        ensure!(
            !self.name.contains(['/', '\\'])
                && matches!(name_components.next(), Some(Utf8Component::Normal(_)))
                && name_components.next().is_none(),
            "repo name `{}` must be one path-safe component",
            self.name
        );
        if let Some(project_root) = &self.project_root {
            ensure!(
                !project_root.as_str().is_empty()
                    && !project_root.is_absolute()
                    && !project_root.as_str().contains(['\\', ':'])
                    && project_root
                        .as_str()
                        .split('/')
                        .all(|segment| !segment.is_empty() && !matches!(segment, "." | "..")),
                "repo `{}` project root `{project_root}` must be a canonical path within its checkout",
                self.name
            );
        }

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
        Manifest::load(path).expect("default corpus manifest should load")
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
                .expect("expected repo should exist in default corpus manifest");
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
            .expect("default corpus manifest should contain the GH-401 fixture");
        assert_eq!(
            fixture
                .django_settings_modules()
                .collect::<Vec<_>>()
                .as_slice(),
            ["site1.settings.dev", "site2.settings.dev"]
        );
    }

    #[test]
    fn manifest_exposes_real_repo_settings_projects() {
        let manifest = load_default_manifest();
        let projects = manifest
            .repo_settings_projects()
            .into_iter()
            .map(|project| {
                (
                    project.repo_name,
                    project.relative_root.map_or(".", Utf8Path::as_str),
                    project.django_settings_modules,
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            projects,
            vec![
                ("archivebox", ".", vec!["archivebox.core.settings"]),
                (
                    "django-allauth",
                    ".",
                    vec!["tests.projects.account_only.settings"]
                ),
                ("healthchecks", ".", vec!["hc.settings"]),
                (
                    "inventree",
                    "src/backend/InvenTree",
                    vec!["InvenTree.settings"]
                ),
                ("netbox", "netbox", vec!["netbox.settings"]),
                ("pretix", ".", vec!["pretix.settings"]),
                ("sentry", ".", vec!["sentry.conf.server"]),
            ]
        );
    }

    #[test]
    fn repo_project_root_must_stay_within_the_checkout() {
        for root in [
            "",
            "/absolute",
            "..",
            "nested/../outside",
            "./nested",
            "nested/.",
            "C:outside",
            "C:\\outside",
            "nested\\root",
        ] {
            let repo = Repo {
                name: "example".to_string(),
                url: "https://example.com/repo.git".to_string(),
                git_ref: None,
                django_settings_module: Some("project.settings".to_string()),
                django_settings_modules: Vec::new(),
                project_root: Some(Utf8PathBuf::from(root)),
            };

            assert!(repo.validate().is_err(), "`{root}` should be rejected");
        }
    }

    #[test]
    fn repo_name_must_be_one_path_safe_component() {
        for name in ["", ".", "..", "nested/repo", "nested\\repo"] {
            let repo = Repo {
                name: name.to_string(),
                url: "https://example.com/repo.git".to_string(),
                git_ref: None,
                django_settings_module: Some("project.settings".to_string()),
                django_settings_modules: Vec::new(),
                project_root: None,
            };

            assert!(repo.validate().is_err(), "`{name}` should be rejected");
        }
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

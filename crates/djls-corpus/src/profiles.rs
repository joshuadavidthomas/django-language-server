use std::collections::BTreeSet;

use camino::Utf8Component;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use serde::Deserialize;

use crate::Corpus;

const PROJECT_MODEL_PROFILES: &str = include_str!("../project-model-profiles.toml");
const PROJECT_MODEL_FIXTURES_DIR: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/project-model");

#[derive(Debug, Clone, Deserialize)]
pub struct ProfileSet {
    #[serde(default, rename = "profile")]
    pub profiles: Vec<Profile>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Profile {
    pub id: String,
    pub description: String,
    pub source: Source,
    #[serde(default)]
    pub source_roots: Vec<String>,
    #[serde(default)]
    pub django_settings_file_patterns: Vec<String>,
    #[serde(default, rename = "django_environment")]
    pub django_environments: Vec<DjangoEnvironmentProfile>,
    #[serde(default)]
    pub expected_union: ExpectedFacts,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Source {
    pub kind: SourceKind,
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Corpus,
    Fixture,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DjangoEnvironmentProfile {
    pub root: String,
    pub settings_file: String,
    pub settings_module: String,
    #[serde(default)]
    pub extends_files: Vec<String>,
    pub installed_apps_confidence: Confidence,
    pub templates_confidence: Confidence,
    #[serde(default)]
    pub expected: ExpectedFacts,
    #[serde(default)]
    pub expected_partial_reasons: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Known,
    Partial,
    Unknown,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ExpectedFacts {
    #[serde(default)]
    pub local_apps: Vec<String>,
    #[serde(default)]
    pub external_apps: Vec<String>,
    #[serde(default)]
    pub unresolved_apps: Vec<String>,
    #[serde(default)]
    pub template_dirs: Vec<String>,
    #[serde(default)]
    pub templatetag_modules: Vec<String>,
}

pub fn project_model_profiles() -> anyhow::Result<ProfileSet> {
    let profiles: ProfileSet = toml::from_str(PROJECT_MODEL_PROFILES)?;
    profiles.validate()?;
    Ok(profiles)
}

impl ProfileSet {
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Profile> {
        self.profiles.iter().find(|profile| profile.id == id)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(!self.profiles.is_empty(), "profile set must not be empty");

        let mut ids = BTreeSet::new();
        for profile in &self.profiles {
            anyhow::ensure!(
                ids.insert(profile.id.as_str()),
                "duplicate profile id `{}`",
                profile.id
            );
            profile.validate()?;
        }

        Ok(())
    }
}

impl Profile {
    #[must_use]
    pub fn root_path(&self, corpus: Option<&Corpus>) -> Option<Utf8PathBuf> {
        match self.source.kind {
            SourceKind::Corpus => corpus.map(|corpus| corpus.root().join(&self.source.path)),
            SourceKind::Fixture => {
                Some(Utf8Path::new(PROJECT_MODEL_FIXTURES_DIR).join(&self.source.path))
            }
        }
    }

    fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(!self.id.trim().is_empty(), "profile id must not be empty");
        anyhow::ensure!(
            !self.description.trim().is_empty(),
            "profile `{}` must have a description",
            self.id
        );
        self.source.validate(&self.id)?;
        anyhow::ensure!(
            !self.source_roots.is_empty(),
            "profile `{}` must define at least one source root",
            self.id
        );
        for source_root in &self.source_roots {
            ensure_relative_path(source_root, &format!("profile `{}` source root", self.id))?;
        }
        for pattern in &self.django_settings_file_patterns {
            ensure_relative_path(
                pattern,
                &format!("profile `{}` Django settings file pattern", self.id),
            )?;
        }
        anyhow::ensure!(
            !self.django_environments.is_empty(),
            "profile `{}` must define at least one Django environment",
            self.id
        );

        let mut roots = BTreeSet::new();
        for environment in &self.django_environments {
            anyhow::ensure!(
                roots.insert(environment.root.as_str()),
                "profile `{}` has duplicate Django environment root `{}`",
                self.id,
                environment.root
            );
            environment.validate(&self.id)?;
        }

        self.expected_union.validate(&self.id, "expected_union")?;

        Ok(())
    }
}

impl Source {
    fn validate(&self, profile_id: &str) -> anyhow::Result<()> {
        ensure_relative_path(&self.path, &format!("profile `{profile_id}` source path"))
    }
}

impl DjangoEnvironmentProfile {
    fn validate(&self, profile_id: &str) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.root.trim().is_empty(),
            "profile `{profile_id}` Django environment root must not be empty"
        );
        ensure_relative_path(
            &self.root,
            &format!("profile `{profile_id}` Django environment root"),
        )?;
        anyhow::ensure!(
            !self.settings_module.trim().is_empty(),
            "profile `{profile_id}` Django environment `{}` must define a settings module",
            self.root
        );
        ensure_relative_path(
            &self.settings_file,
            &format!(
                "profile `{profile_id}` Django environment `{}` settings file",
                self.root
            ),
        )?;
        for path in &self.extends_files {
            ensure_relative_path(
                path,
                &format!(
                    "profile `{profile_id}` Django environment `{}` extends file",
                    self.root
                ),
            )?;
        }
        self.expected.validate(
            profile_id,
            &format!("Django environment `{}` expected facts", self.root),
        )?;
        Ok(())
    }
}

impl ExpectedFacts {
    fn validate(&self, profile_id: &str, field: &str) -> anyhow::Result<()> {
        for path in &self.template_dirs {
            ensure_relative_path(
                path,
                &format!("profile `{profile_id}` {field} template dir"),
            )?;
        }
        Ok(())
    }
}

fn ensure_relative_path(path: &str, field: &str) -> anyhow::Result<()> {
    let path = Utf8Path::new(path);
    anyhow::ensure!(
        !path.as_std_path().is_absolute(),
        "{field} must be relative, got `{path}`"
    );
    anyhow::ensure!(
        !path
            .components()
            .any(|component| matches!(component, Utf8Component::ParentDir)),
        "{field} must not contain `..`, got `{path}`"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_load_and_cover_required_shapes() {
        let profiles = project_model_profiles().unwrap();

        for id in [
            "archivebox",
            "healthchecks",
            "inventree",
            "netbox",
            "pretix",
            "sentry",
            "django-allauth",
            "gh401-multisite-split-settings",
        ] {
            assert!(profiles.get(id).is_some(), "missing profile `{id}`");
        }

        let gh401 = profiles.get("gh401-multisite-split-settings").unwrap();
        assert_eq!(gh401.django_environments.len(), 2);
        assert_eq!(
            gh401.django_settings_file_patterns,
            vec!["projects/*/settings/dev.py"]
        );
        let site1 = gh401
            .django_environments
            .iter()
            .find(|environment| environment.root == "projects/site1")
            .unwrap();
        let site2 = gh401
            .django_environments
            .iter()
            .find(|environment| environment.root == "projects/site2")
            .unwrap();
        assert_eq!(
            site1.expected.local_apps,
            vec!["clientname.app1", "clientname.app2"]
        );
        assert_eq!(
            site2.expected.local_apps,
            vec!["clientname.app2", "clientname.app3"]
        );
        assert_eq!(
            gh401.expected_union.local_apps,
            vec!["clientname.app1", "clientname.app2", "clientname.app3"]
        );
    }

    #[test]
    fn fixture_profile_paths_exist() {
        let profiles = project_model_profiles().unwrap();

        for profile in profiles
            .profiles
            .iter()
            .filter(|profile| profile.source.kind == SourceKind::Fixture)
        {
            let root = profile.root_path(None).unwrap();
            assert_profile_paths_exist(profile, &root);
        }
    }

    #[test]
    fn corpus_profile_paths_exist_when_synced() {
        if !Corpus::is_available() {
            return;
        }

        let corpus = Corpus::require();
        let profiles = project_model_profiles().unwrap();

        for profile in profiles
            .profiles
            .iter()
            .filter(|profile| profile.source.kind == SourceKind::Corpus)
        {
            let root = profile.root_path(Some(&corpus)).unwrap();
            assert_profile_paths_exist(profile, &root);
        }
    }

    fn assert_profile_paths_exist(profile: &Profile, root: &Utf8Path) {
        assert!(root.as_std_path().exists(), "missing profile root `{root}`");

        for source_root in &profile.source_roots {
            assert!(
                root.join(source_root).as_std_path().exists(),
                "profile `{}` source root `{}` does not exist",
                profile.id,
                source_root
            );
        }

        for environment in &profile.django_environments {
            assert!(
                root.join(&environment.settings_file)
                    .as_std_path()
                    .is_file(),
                "profile `{}` Django environment `{}` settings file `{}` does not exist",
                profile.id,
                environment.root,
                environment.settings_file
            );
            for extends_file in &environment.extends_files {
                assert!(
                    root.join(extends_file).as_std_path().is_file(),
                    "profile `{}` Django environment `{}` extends file `{}` does not exist",
                    profile.id,
                    environment.root,
                    extends_file
                );
            }
            assert_expected_paths_exist(profile, root, &environment.expected);
        }

        assert_expected_paths_exist(profile, root, &profile.expected_union);
    }

    fn assert_expected_paths_exist(profile: &Profile, root: &Utf8Path, expected: &ExpectedFacts) {
        for template_dir in &expected.template_dirs {
            assert!(
                root.join(template_dir).as_std_path().is_dir(),
                "profile `{}` template dir `{}` does not exist",
                profile.id,
                template_dir
            );
        }

        for local_app in &expected.local_apps {
            assert!(
                module_exists(root, &profile.source_roots, local_app),
                "profile `{}` local app `{}` does not exist under source roots {:?}",
                profile.id,
                local_app,
                profile.source_roots
            );
        }

        for module in &expected.templatetag_modules {
            assert!(
                module_file_exists(root, &profile.source_roots, module),
                "profile `{}` templatetag module `{}` does not exist under source roots {:?}",
                profile.id,
                module,
                profile.source_roots
            );
        }
    }

    fn module_exists(root: &Utf8Path, source_roots: &[String], module: &str) -> bool {
        let module_path = module.replace('.', "/");
        source_roots.iter().any(|source_root| {
            let root = root.join(source_root);
            root.join(&module_path)
                .join("__init__.py")
                .as_std_path()
                .is_file()
                || root
                    .join(format!("{module_path}.py"))
                    .as_std_path()
                    .is_file()
        })
    }

    fn module_file_exists(root: &Utf8Path, source_roots: &[String], module: &str) -> bool {
        let module_path = format!("{}.py", module.replace('.', "/"));
        source_roots.iter().any(|source_root| {
            root.join(source_root)
                .join(&module_path)
                .as_std_path()
                .is_file()
        })
    }
}

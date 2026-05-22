use camino::Utf8PathBuf;
use djls_conf::Settings;

use crate::load_env_file_outcome;
use crate::DjangoEnvironmentSeed;
use crate::DjangoSettingsModuleSeed;
use crate::Interpreter;
use crate::ProjectDiscoveryIssue;
use crate::ProjectEnvVars;

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectDiscoveryLoadRequest {
    roots: Vec<Utf8PathBuf>,
    client_settings: Settings,
}

impl ProjectDiscoveryLoadRequest {
    #[must_use]
    pub fn new(roots: Vec<Utf8PathBuf>, client_settings: Settings) -> Self {
        Self {
            roots,
            client_settings,
        }
    }

    #[must_use]
    pub fn roots(&self) -> &[Utf8PathBuf] {
        &self.roots
    }

    #[must_use]
    pub fn client_settings(&self) -> &Settings {
        &self.client_settings
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectDiscoverySetData {
    roots: Vec<RootDiscoveryData>,
}

impl ProjectDiscoverySetData {
    #[must_use]
    pub fn new(roots: Vec<RootDiscoveryData>) -> Self {
        Self { roots }
    }

    #[must_use]
    pub fn roots(&self) -> &[RootDiscoveryData] {
        &self.roots
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RootDiscoveryData {
    root: Utf8PathBuf,
    interpreter: Option<Interpreter>,
    settings_module_seed: Option<DjangoSettingsModuleSeed>,
    configured_environment_seeds: Vec<DjangoEnvironmentSeed>,
    pythonpath: Vec<Utf8PathBuf>,
    env_vars: ProjectEnvVars,
    issues: Vec<ProjectDiscoveryIssue>,
}

impl RootDiscoveryData {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        root: Utf8PathBuf,
        interpreter: Option<Interpreter>,
        settings_module_seed: Option<DjangoSettingsModuleSeed>,
        configured_environment_seeds: Vec<DjangoEnvironmentSeed>,
        pythonpath: Vec<Utf8PathBuf>,
        env_vars: ProjectEnvVars,
        issues: Vec<ProjectDiscoveryIssue>,
    ) -> Self {
        Self {
            root,
            interpreter,
            settings_module_seed,
            configured_environment_seeds,
            pythonpath,
            env_vars,
            issues,
        }
    }

    #[must_use]
    pub fn root(&self) -> &Utf8PathBuf {
        &self.root
    }

    #[must_use]
    pub fn interpreter(&self) -> Option<&Interpreter> {
        self.interpreter.as_ref()
    }

    #[must_use]
    pub fn settings_module_seed(&self) -> Option<&DjangoSettingsModuleSeed> {
        self.settings_module_seed.as_ref()
    }

    #[must_use]
    pub fn configured_environment_seeds(&self) -> &[DjangoEnvironmentSeed] {
        &self.configured_environment_seeds
    }

    #[must_use]
    pub fn pythonpath(&self) -> &[Utf8PathBuf] {
        &self.pythonpath
    }

    #[must_use]
    pub fn env_vars(&self) -> &ProjectEnvVars {
        &self.env_vars
    }

    #[must_use]
    pub fn issues(&self) -> &[ProjectDiscoveryIssue] {
        &self.issues
    }
}

#[must_use]
pub fn build_project_discovery_data(
    request: ProjectDiscoveryLoadRequest,
) -> ProjectDiscoverySetData {
    let roots = request
        .roots
        .into_iter()
        .map(|root| root_discovery_data(root, &request.client_settings))
        .collect();
    ProjectDiscoverySetData::new(roots)
}

fn root_discovery_data(root: Utf8PathBuf, client_settings: &Settings) -> RootDiscoveryData {
    let mut issues = Vec::new();
    let settings = match djls_conf::Settings::load(&root, Some(client_settings.clone())) {
        Ok(settings) => settings,
        Err(errors) => {
            issues.extend(errors.into_iter().map(|error| {
                ProjectDiscoveryIssue::ConfigLoadFailed {
                    root: root.clone(),
                    error: error.into(),
                }
            }));
            client_settings.clone()
        }
    };

    let interpreter = Some(Interpreter::discover(settings.venv_path()));
    let settings_module_seed = settings
        .django_settings_module()
        .map(DjangoSettingsModuleSeed::new);
    let configured_environment_seeds = settings
        .django_environments()
        .iter()
        .filter_map(|environment| {
            environment.django_settings_module().map(|settings_module| {
                DjangoEnvironmentSeed::from_settings_module(
                    None,
                    DjangoSettingsModuleSeed::new(settings_module),
                    Some(root.join(environment.root())),
                )
            })
        })
        .collect();
    let pythonpath = settings
        .pythonpath()
        .iter()
        .map(|path| root.join(path))
        .collect();
    let env_outcome = load_env_file_outcome(&root, &settings);
    if let Some(kind) = env_outcome.issue() {
        issues.push(ProjectDiscoveryIssue::EnvFileLoadFailed {
            root: root.clone(),
            source: env_outcome.source().to_owned(),
            kind,
        });
    }
    let (env_entries, duplicate_issues) = resolve_env_vars(
        &root,
        env_outcome.source().to_owned(),
        env_outcome.entries().to_vec(),
    );
    issues.extend(duplicate_issues);
    let env_vars = ProjectEnvVars::from_resolved_entries(env_entries)
        .expect("env var duplicate names should be resolved before construction");

    RootDiscoveryData::new(
        root,
        interpreter,
        settings_module_seed,
        configured_environment_seeds,
        pythonpath,
        env_vars,
        issues,
    )
}

#[allow(clippy::needless_pass_by_value)]
fn resolve_env_vars(
    root: &Utf8PathBuf,
    source: Utf8PathBuf,
    entries: Vec<(String, String)>,
) -> (Vec<(String, String)>, Vec<ProjectDiscoveryIssue>) {
    let mut resolved = std::collections::BTreeMap::new();
    let mut issues = Vec::new();
    for (name, value) in entries {
        if resolved.insert(name.clone(), value).is_some() {
            issues.push(ProjectDiscoveryIssue::DuplicateEnvVar {
                root: root.clone(),
                source: source.clone(),
                name,
            });
        }
    }
    (resolved.into_iter().collect(), issues)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    fn utf8(path: &std::path::Path) -> Utf8PathBuf {
        Utf8PathBuf::from_path_buf(path.to_path_buf()).unwrap()
    }

    #[test]
    fn loading_settings_preserves_config_failure() {
        let dir = tempdir().unwrap();
        let root = utf8(dir.path());
        let source = root.join("djls.toml");
        fs::write(&source, "debug = [true").unwrap();

        let data = build_project_discovery_data(ProjectDiscoveryLoadRequest::new(
            vec![root.clone()],
            Settings::default(),
        ));

        assert_eq!(data.roots().len(), 1);
        assert_eq!(
            data.roots()[0].issues(),
            &[ProjectDiscoveryIssue::ConfigLoadFailed {
                root,
                error: crate::ProjectConfigLoadError::Parse(source),
            }]
        );
    }

    #[test]
    fn loading_settings_lowers_configured_environment_and_pythonpath() {
        let dir = tempdir().unwrap();
        let root = utf8(dir.path());
        fs::write(
            root.join("djls.toml"),
            r#"
django_settings_module = "project.settings"
pythonpath = ["src"]

[[django_environments]]
root = "apps/blog"
django_settings_module = "blog.settings"
"#,
        )
        .unwrap();

        let data = build_project_discovery_data(ProjectDiscoveryLoadRequest::new(
            vec![root.clone()],
            Settings::default(),
        ));
        let root_data = &data.roots()[0];

        assert_eq!(
            root_data
                .settings_module_seed()
                .map(DjangoSettingsModuleSeed::as_str),
            Some("project.settings")
        );
        assert_eq!(root_data.pythonpath(), &[root.join("src")]);
        assert_eq!(root_data.configured_environment_seeds().len(), 1);
        assert_eq!(
            root_data.configured_environment_seeds()[0]
                .settings_module()
                .as_str(),
            "blog.settings"
        );
        assert_eq!(
            root_data.configured_environment_seeds()[0].root(),
            Some(&root.join("apps/blog"))
        );
    }

    #[test]
    fn loading_settings_records_configured_env_file_failure() {
        let dir = tempdir().unwrap();
        let root = utf8(dir.path());
        fs::write(root.join("djls.toml"), "env_file = 'missing.env'").unwrap();

        let data = build_project_discovery_data(ProjectDiscoveryLoadRequest::new(
            vec![root.clone()],
            Settings::default(),
        ));

        assert!(data.roots()[0]
            .issues()
            .contains(&ProjectDiscoveryIssue::EnvFileLoadFailed {
                root: root.clone(),
                source: root.join("missing.env"),
                kind: crate::EnvFileLoadIssueKind::Missing,
            },));
    }

    #[test]
    fn loading_settings_records_duplicate_env_vars_and_keeps_last_value() {
        let dir = tempdir().unwrap();
        let root = utf8(dir.path());
        fs::write(
            root.join(".env"),
            "DJANGO_SETTINGS_MODULE=a\nDJANGO_SETTINGS_MODULE=b\n",
        )
        .unwrap();

        let data = build_project_discovery_data(ProjectDiscoveryLoadRequest::new(
            vec![root.clone()],
            Settings::default(),
        ));

        assert_eq!(
            data.roots()[0].env_vars().entries(),
            &[("DJANGO_SETTINGS_MODULE".to_string(), "b".to_string())]
        );
        assert!(data.roots()[0]
            .issues()
            .contains(&ProjectDiscoveryIssue::DuplicateEnvVar {
                root: root.clone(),
                source: root.join(".env"),
                name: "DJANGO_SETTINGS_MODULE".to_string(),
            },));
    }

    #[test]
    fn loading_settings_canonicalizes_env_vars() {
        let dir = tempdir().unwrap();
        let root = utf8(dir.path());
        fs::write(root.join(".env"), "B=2\nA=1\n").unwrap();

        let data = build_project_discovery_data(ProjectDiscoveryLoadRequest::new(
            vec![root],
            Settings::default(),
        ));

        assert_eq!(
            data.roots()[0].env_vars().entries(),
            &[
                ("A".to_string(), "1".to_string()),
                ("B".to_string(), "2".to_string())
            ]
        );
    }
}

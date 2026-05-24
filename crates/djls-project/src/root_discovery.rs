use std::collections::BTreeSet;

use camino::Utf8PathBuf;
use djls_conf::Settings;

use crate::env::load_env_file_outcome;
use crate::Interpreter;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectRootDiscovery {
    Absent,
    Ready(Vec<ProjectRoot>),
    Unavailable { issues: ProjectRootDiscoveryIssues },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectRootDiscoveryApplyResult {
    Applied {
        discovery: ProjectRootDiscovery,
        has_issues: bool,
    },
    Unavailable(ProjectRootDiscovery),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectRoot {
    root: Utf8PathBuf,
    interpreter: Option<Interpreter>,
    settings_module_seed: Option<DjangoSettingsModuleSeed>,
    configured_environment_seeds: Vec<DjangoEnvironmentSeed>,
    pythonpath: Vec<Utf8PathBuf>,
    env_vars: ProjectEnvVars,
    issues: Vec<ProjectRootDiscoveryIssue>,
}

impl ProjectRoot {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        root: Utf8PathBuf,
        interpreter: Option<Interpreter>,
        settings_module_seed: Option<DjangoSettingsModuleSeed>,
        configured_environment_seeds: Vec<DjangoEnvironmentSeed>,
        pythonpath: Vec<Utf8PathBuf>,
        env_vars: ProjectEnvVars,
        issues: Vec<ProjectRootDiscoveryIssue>,
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
    pub fn issues(&self) -> &[ProjectRootDiscoveryIssue] {
        &self.issues
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DjangoEnvironmentSeed {
    SettingsModule {
        name: Option<String>,
        settings_module: DjangoSettingsModuleSeed,
        root: Option<Utf8PathBuf>,
    },
}

impl DjangoEnvironmentSeed {
    #[must_use]
    pub fn from_settings_module(
        name: Option<String>,
        settings_module: DjangoSettingsModuleSeed,
        root: Option<Utf8PathBuf>,
    ) -> Self {
        Self::SettingsModule {
            name,
            settings_module,
            root,
        }
    }

    #[must_use]
    pub fn name(&self) -> Option<&str> {
        match self {
            Self::SettingsModule { name, .. } => name.as_deref(),
        }
    }

    #[must_use]
    pub fn settings_module(&self) -> &DjangoSettingsModuleSeed {
        match self {
            Self::SettingsModule {
                settings_module, ..
            } => settings_module,
        }
    }

    #[must_use]
    pub fn root(&self) -> Option<&Utf8PathBuf> {
        match self {
            Self::SettingsModule { root, .. } => root.as_ref(),
        }
    }
}

pub type DjangoSettingsModuleSeed = String;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProjectEnvVars(Vec<(String, String)>);

impl ProjectEnvVars {
    pub fn from_resolved_entries(
        mut entries: Vec<(String, String)>,
    ) -> Result<Self, DuplicateEnvVarName> {
        let mut seen = BTreeSet::new();
        for (name, _value) in &entries {
            if !seen.insert(name.clone()) {
                return Err(DuplicateEnvVarName { name: name.clone() });
            }
        }
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(Self(entries))
    }

    #[must_use]
    pub fn entries(&self) -> &[(String, String)] {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DuplicateEnvVarName {
    name: String,
}

impl DuplicateEnvVarName {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectRootDiscoveryIssues(Vec<ProjectRootDiscoveryIssue>);

impl ProjectRootDiscoveryIssues {
    pub fn new(
        issues: Vec<ProjectRootDiscoveryIssue>,
    ) -> Result<Self, EmptyProjectRootDiscoveryIssues> {
        if issues.is_empty() {
            return Err(EmptyProjectRootDiscoveryIssues);
        }
        Ok(Self(issues))
    }

    #[must_use]
    pub fn as_slice(&self) -> &[ProjectRootDiscoveryIssue] {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EmptyProjectRootDiscoveryIssues;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectRootDiscoveryIssue {
    ConfigLoadFailed {
        root: Utf8PathBuf,
        error: ProjectConfigLoadError,
    },
    EnvFileLoadFailed {
        root: Utf8PathBuf,
        source: Utf8PathBuf,
        kind: EnvFileLoadIssueKind,
    },
    DuplicateEnvVar {
        root: Utf8PathBuf,
        source: Utf8PathBuf,
        name: String,
    },
    NoWorkspaceRoots,
    FixtureDoesNotModelDiscovery,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectConfigLoadError {
    Io(Utf8PathBuf),
    Parse(Utf8PathBuf),
    Schema(Option<Utf8PathBuf>),
    Unsupported(Utf8PathBuf),
}

impl From<djls_conf::SettingsLoadError> for ProjectConfigLoadError {
    fn from(error: djls_conf::SettingsLoadError) -> Self {
        match error {
            djls_conf::SettingsLoadError::Io(source_path) => Self::Io(source_path),
            djls_conf::SettingsLoadError::Parse(source_path) => Self::Parse(source_path),
            djls_conf::SettingsLoadError::Schema(source_path) => Self::Schema(source_path),
            djls_conf::SettingsLoadError::Unsupported(source_path) => {
                Self::Unsupported(source_path)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnvFileLoadIssueKind {
    Missing,
    Io,
    Parse,
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use camino::Utf8Path;
    use camino::Utf8PathBuf;
    use djls_source::SourceFiles;

    use super::*;
    use crate::project::Project;
    use crate::Db as ProjectDb;

    #[salsa::db]
    #[derive(Default)]
    struct TestDb {
        storage: salsa::Storage<Self>,
        files: SourceFiles,
        project: OnceLock<Project>,
    }

    impl TestDb {
        fn new_with_project() -> Self {
            let db = Self::default();
            let project = Project::virtual_project(&db);
            db.project
                .set(project)
                .expect("project should initialize once");
            db
        }
    }

    #[salsa::db]
    impl salsa::Database for TestDb {}

    #[salsa::db]
    impl djls_source::Db for TestDb {
        fn files(&self) -> &SourceFiles {
            &self.files
        }

        fn read_file(&self, _path: &Utf8Path) -> std::io::Result<String> {
            Ok(String::new())
        }
    }

    #[salsa::db]
    impl crate::Db for TestDb {
        fn project(&self) -> Project {
            *self.project.get().expect("project should be initialized")
        }
    }

    #[test]
    fn discovery_preserves_multiple_roots_without_primary_selection() {
        let first = ProjectRoot::new(
            Utf8PathBuf::from("/workspace/a"),
            None,
            None,
            Vec::new(),
            Vec::new(),
            ProjectEnvVars::default(),
            Vec::new(),
        );
        let second = ProjectRoot::new(
            Utf8PathBuf::from("/workspace/b"),
            None,
            None,
            Vec::new(),
            Vec::new(),
            ProjectEnvVars::default(),
            Vec::new(),
        );

        let discovery = ProjectRootDiscovery::Ready(vec![first.clone(), second.clone()]);
        let ProjectRootDiscovery::Ready(roots) = discovery else {
            panic!("discovery should be ready");
        };

        assert_eq!(roots, vec![first, second]);
    }

    #[test]
    fn unavailable_discovery_requires_at_least_one_issue() {
        assert_eq!(
            ProjectRootDiscoveryIssues::new(Vec::new()),
            Err(EmptyProjectRootDiscoveryIssues)
        );
        assert!(
            ProjectRootDiscoveryIssues::new(vec![ProjectRootDiscoveryIssue::NoWorkspaceRoots])
                .is_ok()
        );
    }

    #[test]
    fn env_vars_are_canonicalized_after_duplicate_resolution() {
        let vars = ProjectEnvVars::from_resolved_entries(vec![
            (
                "DJANGO_SETTINGS_MODULE".to_string(),
                "project.settings".to_string(),
            ),
            ("A".to_string(), "1".to_string()),
        ])
        .expect("unique env vars should construct");

        assert_eq!(
            vars.entries(),
            &[
                ("A".to_string(), "1".to_string()),
                (
                    "DJANGO_SETTINGS_MODULE".to_string(),
                    "project.settings".to_string()
                ),
            ]
        );
    }

    #[test]
    fn env_vars_reject_duplicate_names_before_canonicalization() {
        let duplicate = ProjectEnvVars::from_resolved_entries(vec![
            (
                "DJANGO_SETTINGS_MODULE".to_string(),
                "a.settings".to_string(),
            ),
            (
                "DJANGO_SETTINGS_MODULE".to_string(),
                "b.settings".to_string(),
            ),
        ])
        .expect_err("duplicate env var names should be resolved before construction");

        assert_eq!(duplicate.name(), "DJANGO_SETTINGS_MODULE");
    }

    #[salsa::tracked]
    fn discovery_root_count(db: &dyn crate::Db) -> Option<usize> {
        match db.project().root_discovery(db) {
            ProjectRootDiscovery::Ready(roots) => Some(roots.len()),
            ProjectRootDiscovery::Absent | ProjectRootDiscovery::Unavailable { .. } => None,
        }
    }

    #[test]
    fn discovery_invalidation_tracks_stable_project_discovery_field() {
        let mut db = TestDb::new_with_project();
        assert_eq!(discovery_root_count(&db), None);

        let root = ProjectRoot::new(
            Utf8PathBuf::from("/workspace"),
            None,
            None,
            Vec::new(),
            Vec::new(),
            ProjectEnvVars::default(),
            Vec::new(),
        );
        let discovery = ProjectRootDiscovery::Ready(vec![root]);
        db.set_project_root_discovery(discovery);

        assert_eq!(discovery_root_count(&db), Some(1));
    }

    #[test]
    fn environment_seed_requires_settings_module() {
        let seed = DjangoEnvironmentSeed::from_settings_module(
            Some("default".to_string()),
            "project.settings".to_string(),
            Some(Utf8PathBuf::from("/workspace")),
        );

        assert_eq!(seed.name(), Some("default"));
        assert_eq!(seed.settings_module().as_str(), "project.settings");
        assert_eq!(seed.root(), Some(&Utf8PathBuf::from("/workspace")));
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ProjectRootDiscoveryLoadRequest {
    roots: Vec<Utf8PathBuf>,
    client_settings: Settings,
}

impl ProjectRootDiscoveryLoadRequest {
    #[must_use]
    pub(crate) fn new(roots: Vec<Utf8PathBuf>, client_settings: Settings) -> Self {
        Self {
            roots,
            client_settings,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectRootDiscoveryUpdate {
    roots: Vec<ProjectRoot>,
}

impl ProjectRootDiscoveryUpdate {
    #[must_use]
    pub fn new(roots: Vec<ProjectRoot>) -> Self {
        Self { roots }
    }

    #[must_use]
    pub fn roots(&self) -> &[ProjectRoot] {
        &self.roots
    }
}

#[must_use]
pub(crate) fn load_project_root_discovery(
    request: ProjectRootDiscoveryLoadRequest,
) -> ProjectRootDiscoveryUpdate {
    let roots = request
        .roots
        .into_iter()
        .map(|root| root_discovery_data(root, &request.client_settings))
        .collect();
    ProjectRootDiscoveryUpdate::new(roots)
}

fn root_discovery_data(root: Utf8PathBuf, client_settings: &Settings) -> ProjectRoot {
    let mut issues = Vec::new();
    let settings = match djls_conf::Settings::load(&root, Some(client_settings.clone())) {
        Ok(settings) => settings,
        Err(errors) => {
            issues.extend(errors.into_iter().map(|error| {
                ProjectRootDiscoveryIssue::ConfigLoadFailed {
                    root: root.clone(),
                    error: error.into(),
                }
            }));
            client_settings.clone()
        }
    };

    let interpreter = Some(Interpreter::discover(settings.venv_path()));
    let settings_module_seed = settings.django_settings_module().map(ToString::to_string);
    let configured_environment_seeds = settings
        .django_environments()
        .iter()
        .filter_map(|environment| {
            environment.django_settings_module().map(|settings_module| {
                DjangoEnvironmentSeed::from_settings_module(
                    None,
                    settings_module.to_string(),
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
        issues.push(ProjectRootDiscoveryIssue::EnvFileLoadFailed {
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

    ProjectRoot::new(
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
) -> (Vec<(String, String)>, Vec<ProjectRootDiscoveryIssue>) {
    let mut resolved = std::collections::BTreeMap::new();
    let mut issues = Vec::new();
    for (name, value) in entries {
        if resolved.insert(name.clone(), value).is_some() {
            issues.push(ProjectRootDiscoveryIssue::DuplicateEnvVar {
                root: root.clone(),
                source: source.clone(),
                name,
            });
        }
    }
    (resolved.into_iter().collect(), issues)
}

#[cfg(test)]
mod settings_tests {
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

        let data = load_project_root_discovery(ProjectRootDiscoveryLoadRequest::new(
            vec![root.clone()],
            Settings::default(),
        ));

        assert_eq!(data.roots().len(), 1);
        assert_eq!(
            data.roots()[0].issues(),
            &[ProjectRootDiscoveryIssue::ConfigLoadFailed {
                root,
                error: crate::root_discovery::ProjectConfigLoadError::Parse(source),
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

        let data = load_project_root_discovery(ProjectRootDiscoveryLoadRequest::new(
            vec![root.clone()],
            Settings::default(),
        ));
        let root_data = &data.roots()[0];

        assert_eq!(
            root_data.settings_module_seed().map(String::as_str),
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

        let data = load_project_root_discovery(ProjectRootDiscoveryLoadRequest::new(
            vec![root.clone()],
            Settings::default(),
        ));

        assert!(data.roots()[0]
            .issues()
            .contains(&ProjectRootDiscoveryIssue::EnvFileLoadFailed {
                root: root.clone(),
                source: root.join("missing.env"),
                kind: crate::root_discovery::EnvFileLoadIssueKind::Missing,
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

        let data = load_project_root_discovery(ProjectRootDiscoveryLoadRequest::new(
            vec![root.clone()],
            Settings::default(),
        ));

        assert_eq!(
            data.roots()[0].env_vars().entries(),
            &[("DJANGO_SETTINGS_MODULE".to_string(), "b".to_string())]
        );
        assert!(data.roots()[0]
            .issues()
            .contains(&ProjectRootDiscoveryIssue::DuplicateEnvVar {
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

        let data = load_project_root_discovery(ProjectRootDiscoveryLoadRequest::new(
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

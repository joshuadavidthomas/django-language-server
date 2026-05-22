use std::collections::BTreeSet;

use camino::Utf8PathBuf;

use crate::Interpreter;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectDiscovery {
    Absent,
    Ready(ProjectDiscoverySet),
    Unavailable { issues: ProjectDiscoveryIssues },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectDiscoveryApplyResult {
    Applied(ProjectDiscovery),
    Unavailable(ProjectDiscovery),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectDiscoverySet {
    roots: Vec<RootDiscoveryInput>,
}

impl ProjectDiscoverySet {
    pub fn new(roots: Vec<RootDiscoveryInput>) -> Result<Self, ProjectDiscoverySetError> {
        if roots.is_empty() {
            return Err(ProjectDiscoverySetError::NoWorkspaceRoots);
        }
        Ok(Self { roots })
    }

    #[must_use]
    pub fn roots(&self) -> &[RootDiscoveryInput] {
        &self.roots
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectDiscoverySetError {
    NoWorkspaceRoots,
}

#[salsa::input]
#[derive(Debug)]
pub struct RootDiscoveryInput {
    #[returns(ref)]
    pub root: Utf8PathBuf,
    #[returns(ref)]
    pub interpreter: Option<Interpreter>,
    #[returns(ref)]
    pub settings_module_seed: Option<DjangoSettingsModuleSeed>,
    #[returns(ref)]
    pub configured_environment_seeds: Vec<DjangoEnvironmentSeed>,
    #[returns(ref)]
    pub pythonpath: Vec<Utf8PathBuf>,
    #[returns(ref)]
    pub env_vars: ProjectEnvVars,
    #[returns(ref)]
    pub issues: Vec<ProjectDiscoveryIssue>,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DjangoSettingsModuleSeed(String);

impl DjangoSettingsModuleSeed {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

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
pub struct ProjectDiscoveryIssues(Vec<ProjectDiscoveryIssue>);

impl ProjectDiscoveryIssues {
    pub fn new(issues: Vec<ProjectDiscoveryIssue>) -> Result<Self, EmptyProjectDiscoveryIssues> {
        if issues.is_empty() {
            return Err(EmptyProjectDiscoveryIssues);
        }
        Ok(Self(issues))
    }

    #[must_use]
    pub fn as_slice(&self) -> &[ProjectDiscoveryIssue] {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EmptyProjectDiscoveryIssues;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectDiscoveryIssue {
    ConfigLoadFailed {
        root: Utf8PathBuf,
        source: Option<Utf8PathBuf>,
        kind: ConfigLoadIssueKind,
    },
    ConfigFallbackUsed {
        root: Utf8PathBuf,
        source: Option<Utf8PathBuf>,
    },
    InterpreterDiscoveryFailed {
        root: Utf8PathBuf,
        kind: InterpreterDiscoveryIssueKind,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigLoadIssueKind {
    Io,
    Parse,
    Schema,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterpreterDiscoveryIssueKind {
    NotFound,
    InvalidPath,
    ExecutionFailed,
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
    use crate::Db as ProjectDb;
    use crate::Project;

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
    fn discovery_set_preserves_multiple_roots_without_primary_selection() {
        let db = TestDb::default();
        let first = RootDiscoveryInput::new(
            &db,
            Utf8PathBuf::from("/workspace/a"),
            None,
            None,
            Vec::new(),
            Vec::new(),
            ProjectEnvVars::default(),
            Vec::new(),
        );
        let second = RootDiscoveryInput::new(
            &db,
            Utf8PathBuf::from("/workspace/b"),
            None,
            None,
            Vec::new(),
            Vec::new(),
            ProjectEnvVars::default(),
            Vec::new(),
        );

        let discovery = ProjectDiscovery::Ready(
            ProjectDiscoverySet::new(vec![first, second])
                .expect("non-empty roots should construct discovery set"),
        );
        let ProjectDiscovery::Ready(set) = discovery else {
            panic!("discovery should be ready");
        };

        assert_eq!(set.roots(), &[first, second]);
    }

    #[test]
    fn discovery_set_rejects_empty_roots() {
        assert_eq!(
            ProjectDiscoverySet::new(Vec::new()),
            Err(ProjectDiscoverySetError::NoWorkspaceRoots)
        );
    }

    #[test]
    fn unavailable_discovery_requires_at_least_one_issue() {
        assert_eq!(
            ProjectDiscoveryIssues::new(Vec::new()),
            Err(EmptyProjectDiscoveryIssues)
        );
        assert!(ProjectDiscoveryIssues::new(vec![ProjectDiscoveryIssue::NoWorkspaceRoots]).is_ok());
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
        match db.project().discovery(db) {
            ProjectDiscovery::Ready(discovery) => Some(discovery.roots().len()),
            ProjectDiscovery::Absent | ProjectDiscovery::Unavailable { .. } => None,
        }
    }

    #[test]
    fn discovery_invalidation_tracks_stable_project_discovery_field() {
        let mut db = TestDb::new_with_project();
        assert_eq!(discovery_root_count(&db), None);

        let root = RootDiscoveryInput::new(
            &db,
            Utf8PathBuf::from("/workspace"),
            None,
            None,
            Vec::new(),
            Vec::new(),
            ProjectEnvVars::default(),
            Vec::new(),
        );
        let discovery = ProjectDiscovery::Ready(
            ProjectDiscoverySet::new(vec![root]).expect("root should construct discovery set"),
        );
        db.set_project_discovery(discovery);

        assert_eq!(discovery_root_count(&db), Some(1));
    }

    #[test]
    fn environment_seed_requires_settings_module() {
        let seed = DjangoEnvironmentSeed::from_settings_module(
            Some("default".to_string()),
            DjangoSettingsModuleSeed::new("project.settings"),
            Some(Utf8PathBuf::from("/workspace")),
        );

        assert_eq!(seed.name(), Some("default"));
        assert_eq!(seed.settings_module().as_str(), "project.settings");
        assert_eq!(seed.root(), Some(&Utf8PathBuf::from("/workspace")));
    }
}

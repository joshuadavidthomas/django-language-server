use camino::Utf8Path;
use djls_source::File;

use crate::settings_candidates;
use crate::Db;
use crate::Origin;
use crate::Project;
use crate::ProjectDiscovery;
use crate::ProjectSourceInventory;
use crate::SettingsCandidate;
use crate::SettingsCandidateIssue;
use crate::SettingsCandidateOutcome;
use crate::SettingsCandidateSource;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DjangoEnvironmentId(String);

impl DjangoEnvironmentId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DjangoEnvironmentCandidate {
    id: DjangoEnvironmentId,
    settings: crate::PyModuleName,
    root: Option<camino::Utf8PathBuf>,
    source: EnvironmentCandidateSource,
}

impl DjangoEnvironmentCandidate {
    #[cfg(test)]
    #[must_use]
    pub(crate) fn for_test() -> Self {
        Self {
            id: DjangoEnvironmentId("test:config:/workspace".to_string()),
            settings: crate::PyModuleName::parse("test.settings")
                .expect("test module should be valid"),
            root: Some(camino::Utf8PathBuf::from("/workspace")),
            source: EnvironmentCandidateSource::ExplicitConfig,
        }
    }

    #[must_use]
    pub fn id(&self) -> &DjangoEnvironmentId {
        &self.id
    }

    #[must_use]
    pub fn settings(&self) -> &crate::PyModuleName {
        &self.settings
    }

    #[must_use]
    pub fn root(&self) -> Option<&Utf8Path> {
        self.root.as_deref()
    }

    #[must_use]
    pub fn source(&self) -> &EnvironmentCandidateSource {
        &self.source
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum EnvironmentCandidateSource {
    ExplicitConfig,
    ConfiguredEnvironment,
    EnvironmentVariable,
    ManagePyDefault,
    ConventionalModule,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DjangoEnvironmentCandidatesOutcome {
    Ready {
        candidates: Vec<DjangoEnvironmentCandidate>,
        issues: Vec<EnvironmentCandidatesIssue>,
    },
    Ambiguous {
        candidates: Vec<DjangoEnvironmentCandidate>,
        issues: Vec<EnvironmentCandidatesIssue>,
    },
    Unavailable {
        issue: EnvironmentCandidatesIssue,
    },
    Deferred {
        issue: EnvironmentCandidatesIssue,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EnvironmentCandidatesIssue {
    NoSettingsCandidates,
    SettingsCandidatesUnavailable { issues: Vec<SettingsCandidateIssue> },
    SettingsCandidateIssues { issues: Vec<SettingsCandidateIssue> },
    AmbiguousSettingsCandidates,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EnvironmentSelection {
    Selected(DjangoEnvironmentId),
    Ambiguous {
        candidates: Vec<DjangoEnvironmentCandidate>,
        issues: Vec<EnvironmentSelectionIssue>,
    },
    Unknown {
        issues: Vec<EnvironmentSelectionIssue>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EnvironmentSelectionIssue {
    NoEnvironmentCandidates,
    MultipleCandidatesForFile,
    NoCandidateForFile,
    CandidatesUnavailable { issue: EnvironmentCandidatesIssue },
    CandidatesDeferred { issue: EnvironmentCandidatesIssue },
}

#[salsa::tracked(returns(ref))]
pub fn django_environment_candidates(
    db: &dyn Db,
    project: Project,
) -> DjangoEnvironmentCandidatesOutcome {
    let SettingsCandidateOutcome::Ready { candidates, issues } = settings_candidates(db, project);
    let env_candidates = candidates
        .iter()
        .map(|candidate| environment_candidate(db, project, candidate))
        .collect::<Vec<_>>();

    if env_candidates.is_empty() {
        if issues.is_empty() {
            return DjangoEnvironmentCandidatesOutcome::Deferred {
                issue: EnvironmentCandidatesIssue::NoSettingsCandidates,
            };
        }
        return DjangoEnvironmentCandidatesOutcome::Unavailable {
            issue: EnvironmentCandidatesIssue::SettingsCandidatesUnavailable {
                issues: issues.clone(),
            },
        };
    }

    DjangoEnvironmentCandidatesOutcome::Ready {
        candidates: env_candidates,
        issues: environment_issues_from_settings_issues(issues),
    }
}

#[salsa::tracked(returns(ref))]
pub fn environment_for_file(db: &dyn Db, project: Project, file: File) -> EnvironmentSelection {
    let path = file.path(db);
    let candidates = match django_environment_candidates(db, project) {
        DjangoEnvironmentCandidatesOutcome::Ready { candidates, .. }
        | DjangoEnvironmentCandidatesOutcome::Ambiguous { candidates, .. } => candidates,
        DjangoEnvironmentCandidatesOutcome::Unavailable { issue } => {
            return EnvironmentSelection::Unknown {
                issues: vec![EnvironmentSelectionIssue::CandidatesUnavailable {
                    issue: issue.clone(),
                }],
            };
        }
        DjangoEnvironmentCandidatesOutcome::Deferred { issue } => {
            return EnvironmentSelection::Unknown {
                issues: vec![EnvironmentSelectionIssue::CandidatesDeferred {
                    issue: issue.clone(),
                }],
            };
        }
    };
    let mut matching = candidates
        .iter()
        .filter(|candidate| candidate.root().is_some_and(|root| path.starts_with(root)))
        .cloned()
        .collect::<Vec<_>>();
    matching.sort_by(|left, right| {
        right
            .root
            .as_ref()
            .map(|root| root.as_str().len())
            .cmp(&left.root.as_ref().map(|root| root.as_str().len()))
    });

    let Some(best_len) = matching
        .first()
        .and_then(|candidate| candidate.root.as_ref())
        .map(|root| root.as_str().len())
    else {
        return EnvironmentSelection::Unknown {
            issues: vec![EnvironmentSelectionIssue::NoCandidateForFile],
        };
    };
    matching.retain(|candidate| {
        candidate
            .root
            .as_ref()
            .is_some_and(|root| root.as_str().len() == best_len)
    });

    if matching.len() == 1 {
        return EnvironmentSelection::Selected(matching.remove(0).id);
    }

    EnvironmentSelection::Ambiguous {
        candidates: matching,
        issues: vec![EnvironmentSelectionIssue::MultipleCandidatesForFile],
    }
}

fn environment_candidate(
    db: &dyn Db,
    project: Project,
    candidate: &SettingsCandidate,
) -> DjangoEnvironmentCandidate {
    let root = candidate_root(db, project, candidate);
    let source = match candidate.source() {
        SettingsCandidateSource::ExplicitConfig => EnvironmentCandidateSource::ExplicitConfig,
        SettingsCandidateSource::ConfiguredEnvironment => {
            EnvironmentCandidateSource::ConfiguredEnvironment
        }
        SettingsCandidateSource::EnvironmentVariable => {
            EnvironmentCandidateSource::EnvironmentVariable
        }
        SettingsCandidateSource::ManagePyDefault => EnvironmentCandidateSource::ManagePyDefault,
        SettingsCandidateSource::ConventionalModule => {
            EnvironmentCandidateSource::ConventionalModule
        }
    };
    DjangoEnvironmentCandidate {
        id: DjangoEnvironmentId(format!(
            "{}:{}:{}",
            candidate.module().as_str(),
            source_slug(&source),
            root.as_ref().map_or("", |root| root.as_str()),
        )),
        settings: candidate.module().clone(),
        root,
        source,
    }
}

fn candidate_root(
    db: &dyn Db,
    project: Project,
    candidate: &SettingsCandidate,
) -> Option<camino::Utf8PathBuf> {
    for origin in candidate.origin().origins() {
        match origin {
            Origin::Config { root }
            | Origin::Environment { root, .. }
            | Origin::ConfiguredEnvironment {
                root: Some(root), ..
            } => return Some(root.clone()),
            Origin::PythonSource { file } | Origin::Convention { file } => {
                return owning_root_for_path(db, project, file.path(db));
            }
            Origin::ConfiguredEnvironment { root: None, .. } => {}
        }
    }
    candidate
        .file()
        .and_then(|file| owning_root_for_path(db, project, file.path(db)))
}

fn owning_root_for_path(
    db: &dyn Db,
    project: Project,
    path: &Utf8Path,
) -> Option<camino::Utf8PathBuf> {
    let mut roots = Vec::new();
    if let ProjectSourceInventory::Ready(files) = project.source_inventory(db) {
        roots.extend(
            files
                .merged()
                .data(db)
                .roots()
                .iter()
                .map(|entry| entry.root().path().to_owned()),
        );
    }
    if let ProjectDiscovery::Ready(discovery) = project.discovery(db) {
        roots.extend(discovery.roots().iter().map(|root| root.root(db).clone()));
    }
    roots
        .into_iter()
        .filter(|root| path.starts_with(root.as_path()))
        .max_by_key(|root| root.as_str().len())
}

fn environment_issues_from_settings_issues(
    issues: &[SettingsCandidateIssue],
) -> Vec<EnvironmentCandidatesIssue> {
    if issues.is_empty() {
        Vec::new()
    } else {
        vec![EnvironmentCandidatesIssue::SettingsCandidateIssues {
            issues: issues.to_vec(),
        }]
    }
}

fn source_slug(source: &EnvironmentCandidateSource) -> &'static str {
    match source {
        EnvironmentCandidateSource::ExplicitConfig => "config",
        EnvironmentCandidateSource::ConfiguredEnvironment => "environment",
        EnvironmentCandidateSource::EnvironmentVariable => "env",
        EnvironmentCandidateSource::ManagePyDefault => "manage",
        EnvironmentCandidateSource::ConventionalModule => "convention",
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::OnceLock;

    use camino::Utf8Path;
    use camino::Utf8PathBuf;
    use djls_source::Db as SourceDb;
    use djls_source::FileRootKind;
    use djls_source::LoadedSourceFile;
    use djls_source::SourceFileSet;
    use djls_source::SourceFileSetData;
    use djls_source::SourceFiles;
    use djls_source::SourceRoot;
    use djls_source::SourceRootEntry;
    use djls_source::SourceRootId;
    use rustc_hash::FxHashMap;
    use salsa::Database;

    use super::*;
    use crate::DjangoEnvironmentSeed;
    use crate::DjangoSettingsModuleSeed;
    use crate::ProjectDiscovery;
    use crate::ProjectDiscoverySet;
    use crate::ProjectEnrichment;
    use crate::ProjectEnvVars;
    use crate::ProjectSourceFilesIssue;
    use crate::ProjectSourceInventory;
    use crate::ReadyProjectSourceFiles;
    use crate::RootDiscoveryInput;

    #[salsa::db]
    struct TestDb {
        storage: salsa::Storage<Self>,
        files: SourceFiles,
        sources: FxHashMap<Utf8PathBuf, String>,
        project: OnceLock<Project>,
        events: Arc<Mutex<Vec<salsa::Event>>>,
    }

    impl Default for TestDb {
        fn default() -> Self {
            let events = Arc::new(Mutex::new(Vec::new()));
            let storage = salsa::Storage::new(Some(Box::new({
                let events = Arc::clone(&events);
                move |event| {
                    events
                        .lock()
                        .expect("event log is not poisoned")
                        .push(event)
                }
            })));
            Self {
                storage,
                files: SourceFiles::default(),
                sources: FxHashMap::default(),
                project: OnceLock::new(),
                events,
            }
        }
    }

    #[salsa::db]
    impl salsa::Database for TestDb {}

    #[salsa::db]
    impl djls_source::Db for TestDb {
        fn files(&self) -> &SourceFiles {
            &self.files
        }

        fn read_file(&self, path: &Utf8Path) -> std::io::Result<String> {
            Ok(self.sources.get(path).cloned().unwrap_or_default())
        }
    }

    #[salsa::db]
    impl crate::Db for TestDb {
        fn project(&self) -> Project {
            *self.project.get().expect("test project initialized")
        }
    }

    impl TestDb {
        fn with_project() -> Self {
            let db = Self::default();
            db.project
                .set(Project::new(
                    &db,
                    ProjectSourceInventory::Unavailable {
                        issue: ProjectSourceFilesIssue::NotLoaded,
                    },
                    ProjectDiscovery::Absent,
                    ProjectEnrichment::Absent,
                ))
                .expect("project should initialize once");
            db
        }

        fn set_file(&mut self, path: &str, source: &str) -> File {
            let path = Utf8PathBuf::from(path);
            self.sources.insert(path.clone(), source.to_string());
            self.get_or_create_file(path.as_path())
        }

        fn take_events(&self) -> Vec<salsa::Event> {
            std::mem::take(&mut *self.events.lock().expect("event log is not poisoned"))
        }

        fn tracked_query_executed(&self, events: &[salsa::Event], query_name: &str) -> bool {
            events.iter().any(|event| match &event.kind {
                salsa::EventKind::WillExecute { database_key } => self
                    .ingredient_debug_name(database_key.ingredient_index())
                    .contains(query_name),
                _ => false,
            })
        }
    }

    fn ready_inventory(db: &TestDb, paths: &[&str]) -> ProjectSourceInventory {
        let root_path = Utf8PathBuf::from("/workspace");
        let root_id = SourceRootId::new(root_path.clone());
        let root = SourceRoot::new(root_id.clone(), root_path, FileRootKind::Project);
        let roots = vec![SourceRootEntry::new(root)];
        let files = paths
            .iter()
            .map(|path| {
                let path = Utf8PathBuf::from(path);
                LoadedSourceFile::new(path.clone(), root_id.clone(), db.get_or_create_file(&path))
            })
            .collect::<Vec<_>>();
        let data = SourceFileSetData::new(roots, files).expect("test data should be valid");
        ProjectSourceInventory::Ready(ReadyProjectSourceFiles::merged_for_test(
            SourceFileSet::new(db, data),
        ))
    }

    fn discovery_root(
        db: &TestDb,
        root: &str,
        settings: Option<&str>,
        environments: Vec<DjangoEnvironmentSeed>,
    ) -> RootDiscoveryInput {
        RootDiscoveryInput::new(
            db,
            Utf8PathBuf::from(root),
            None,
            settings.map(DjangoSettingsModuleSeed::new),
            environments,
            Vec::new(),
            ProjectEnvVars::default(),
            Vec::new(),
        )
    }

    #[test]
    fn environments_create_candidates_from_every_settings_candidate_source() {
        let mut db = TestDb::with_project();
        db.set_file(
            "/workspace/manage.py",
            "import os\nos.environ.setdefault('DJANGO_SETTINGS_MODULE', 'manage.settings')\n",
        );
        db.set_file("/workspace/config/settings.py", "SECRET_KEY = 'x'\n");
        db.set_project_source_inventory(ready_inventory(
            &db,
            &["/workspace/manage.py", "/workspace/config/settings.py"],
        ));
        let root = RootDiscoveryInput::new(
            &db,
            Utf8PathBuf::from("/workspace"),
            None,
            Some(DjangoSettingsModuleSeed::new("explicit.settings")),
            vec![DjangoEnvironmentSeed::from_settings_module(
                Some("default".to_string()),
                DjangoSettingsModuleSeed::new("environment.settings"),
                Some(Utf8PathBuf::from("/workspace")),
            )],
            Vec::new(),
            ProjectEnvVars::from_resolved_entries(vec![(
                "DJANGO_SETTINGS_MODULE".to_string(),
                "env.settings".to_string(),
            )])
            .expect("env vars should be valid"),
            Vec::new(),
        );
        db.set_project_discovery(ProjectDiscovery::Ready(
            ProjectDiscoverySet::new(vec![root]).expect("root should create discovery"),
        ));

        let DjangoEnvironmentCandidatesOutcome::Ready { candidates, issues } =
            django_environment_candidates(&db, db.project())
        else {
            panic!("multiple discoverable candidates should be ready");
        };
        assert!(issues.is_empty());
        let sources = candidates
            .iter()
            .map(|candidate| candidate.source().clone())
            .collect::<std::collections::BTreeSet<_>>();

        assert_eq!(sources.len(), 5);
    }

    #[test]
    fn environments_preserve_settings_candidate_issues_with_valid_candidates() {
        let mut db = TestDb::with_project();
        db.set_project_source_inventory(ready_inventory(&db, &[]));
        let root = RootDiscoveryInput::new(
            &db,
            Utf8PathBuf::from("/workspace"),
            None,
            Some(DjangoSettingsModuleSeed::new("explicit.settings")),
            Vec::new(),
            Vec::new(),
            ProjectEnvVars::from_resolved_entries(vec![(
                "DJANGO_SETTINGS_MODULE".to_string(),
                "not a module".to_string(),
            )])
            .expect("env vars should be valid"),
            Vec::new(),
        );
        db.set_project_discovery(ProjectDiscovery::Ready(
            ProjectDiscoverySet::new(vec![root]).expect("root should create discovery"),
        ));

        let DjangoEnvironmentCandidatesOutcome::Ready { candidates, issues } =
            django_environment_candidates(&db, db.project())
        else {
            panic!("valid candidates should remain ready with issues");
        };

        assert_eq!(candidates.len(), 1);
        assert!(matches!(
            issues.as_slice(),
            [EnvironmentCandidatesIssue::SettingsCandidateIssues { .. }]
        ));
    }

    #[test]
    fn conventional_environment_root_is_project_root_not_settings_package() {
        let mut db = TestDb::with_project();
        let file = db.set_file("/workspace/templates/index.html", "hi");
        db.set_file("/workspace/config/settings.py", "SECRET_KEY = 'x'\n");
        db.set_project_source_inventory(ready_inventory(
            &db,
            &[
                "/workspace/templates/index.html",
                "/workspace/config/settings.py",
            ],
        ));

        let EnvironmentSelection::Selected(selected) =
            environment_for_file(&db, db.project(), file)
        else {
            panic!("template outside config package should select conventional environment");
        };

        assert!(selected.as_str().contains("config.settings"));
    }

    #[test]
    fn environment_candidates_reuse_after_loading_environment_discovery_ready() {
        let mut db = TestDb::with_project();
        let root = discovery_root(&db, "/workspace", Some("project.settings"), Vec::new());
        db.set_project_source_inventory(ready_inventory(&db, &[]));
        db.set_project_discovery(ProjectDiscovery::Ready(
            ProjectDiscoverySet::new(vec![root]).expect("root should create discovery"),
        ));

        let DjangoEnvironmentCandidatesOutcome::Ready { candidates, .. } =
            django_environment_candidates(&db, db.project())
        else {
            panic!("single environment candidate should be ready");
        };
        assert_eq!(candidates.len(), 1);
        let _ = db.take_events();

        let DjangoEnvironmentCandidatesOutcome::Ready { candidates, .. } =
            django_environment_candidates(&db, db.project())
        else {
            panic!("environment candidates should be reused");
        };
        assert_eq!(candidates.len(), 1);
        let events = db.take_events();

        assert!(!db.tracked_query_executed(&events, "django_environment_candidates"));
    }

    #[test]
    fn multisite_environment_selection_uses_file_root_prefix() {
        let mut db = TestDb::with_project();
        let file = db.set_file("/workspace/site_b/templates/index.html", "hi");
        db.set_project_source_inventory(ready_inventory(
            &db,
            &["/workspace/site_b/templates/index.html"],
        ));
        let site_a = discovery_root(
            &db,
            "/workspace/site_a",
            Some("site_a.settings"),
            Vec::new(),
        );
        let site_b = discovery_root(
            &db,
            "/workspace/site_b",
            Some("site_b.settings"),
            Vec::new(),
        );
        db.set_project_discovery(ProjectDiscovery::Ready(
            ProjectDiscoverySet::new(vec![site_a, site_b]).expect("roots should create discovery"),
        ));

        let EnvironmentSelection::Selected(selected) =
            environment_for_file(&db, db.project(), file)
        else {
            panic!("site_b template should select site_b environment");
        };

        assert!(selected.as_str().contains("site_b.settings"));
    }
}

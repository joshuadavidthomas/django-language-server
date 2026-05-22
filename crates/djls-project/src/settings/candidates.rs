use djls_source::File;

use crate::module_name_for_path;
use crate::project_layout_index;
use crate::Db;
use crate::DjangoSettingsModuleSeed;
use crate::Origin;
use crate::OriginSet;
use crate::Project;
use crate::ProjectDiscovery;
use crate::ProjectLayoutIndex;
use crate::ProjectLayoutIndexOutcome;
use crate::ProjectLayoutIssue;
use crate::PyModuleName;
use crate::PythonSourceModelStatus;
use crate::StaticValue;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettingsCandidate {
    module: PyModuleName,
    file: Option<File>,
    source: SettingsCandidateSource,
    origin: OriginSet,
}

impl SettingsCandidate {
    #[must_use]
    pub fn new(
        module: PyModuleName,
        file: Option<File>,
        source: SettingsCandidateSource,
        origin: OriginSet,
    ) -> Self {
        Self {
            module,
            file,
            source,
            origin,
        }
    }

    #[must_use]
    pub fn module(&self) -> &PyModuleName {
        &self.module
    }

    #[must_use]
    pub fn file(&self) -> Option<File> {
        self.file
    }

    #[must_use]
    pub fn source(&self) -> &SettingsCandidateSource {
        &self.source
    }

    #[must_use]
    pub fn origin(&self) -> &OriginSet {
        &self.origin
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum SettingsCandidateSource {
    ExplicitConfig,
    ConfiguredEnvironment,
    EnvironmentVariable,
    ManagePyDefault,
    ConventionalModule,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SettingsCandidateOutcome {
    Ready {
        candidates: Vec<SettingsCandidate>,
        issues: Vec<SettingsCandidateIssue>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SettingsCandidateIssue {
    LayoutUnavailable {
        issue: ProjectLayoutIssue,
    },
    InvalidModuleName {
        source: SettingsCandidateSource,
        value: String,
        origin: OriginSet,
    },
}

#[salsa::tracked(returns(ref))]
pub fn settings_candidates(db: &dyn Db, project: Project) -> SettingsCandidateOutcome {
    let mut candidates = Vec::new();
    let mut issues = Vec::new();
    collect_discovery_candidates(db, project, &mut candidates, &mut issues);

    match project_layout_index(db, project) {
        ProjectLayoutIndexOutcome::Ready(layout) => {
            collect_manage_py_candidates(db, layout, &mut candidates, &mut issues);
            collect_conventional_candidates(db, project, layout, &mut candidates);
        }
        ProjectLayoutIndexOutcome::Absent { issue }
        | ProjectLayoutIndexOutcome::Unavailable { issue } => {
            issues.push(SettingsCandidateIssue::LayoutUnavailable {
                issue: issue.clone(),
            });
        }
    }

    candidates.sort_by(|left, right| {
        source_rank(&left.source)
            .cmp(&source_rank(&right.source))
            .then_with(|| left.module.as_str().cmp(right.module.as_str()))
    });

    SettingsCandidateOutcome::Ready { candidates, issues }
}

fn source_rank(source: &SettingsCandidateSource) -> u8 {
    match source {
        SettingsCandidateSource::ExplicitConfig => 0,
        SettingsCandidateSource::ConfiguredEnvironment => 1,
        SettingsCandidateSource::EnvironmentVariable => 2,
        SettingsCandidateSource::ManagePyDefault => 3,
        SettingsCandidateSource::ConventionalModule => 4,
    }
}

fn collect_discovery_candidates(
    db: &dyn Db,
    project: Project,
    candidates: &mut Vec<SettingsCandidate>,
    issues: &mut Vec<SettingsCandidateIssue>,
) {
    let ProjectDiscovery::Ready(discovery) = project.discovery(db) else {
        return;
    };

    for root in discovery.roots() {
        if let Some(seed) = root.settings_module_seed(db) {
            push_seed(
                candidates,
                seed,
                None,
                SettingsCandidateSource::ExplicitConfig,
                OriginSet::single(Origin::Config {
                    root: root.root(db).clone(),
                }),
                issues,
            );
        }
        for environment in root.configured_environment_seeds(db) {
            let seed = environment.settings_module();
            push_seed(
                candidates,
                seed,
                None,
                SettingsCandidateSource::ConfiguredEnvironment,
                OriginSet::single(Origin::ConfiguredEnvironment {
                    root: environment.root().cloned(),
                    name: environment.name().map(str::to_string),
                }),
                issues,
            );
        }
        for (name, value) in root.env_vars(db).entries() {
            if name == "DJANGO_SETTINGS_MODULE" {
                push_value(
                    candidates,
                    value,
                    None,
                    SettingsCandidateSource::EnvironmentVariable,
                    OriginSet::single(Origin::Environment {
                        root: root.root(db).clone(),
                        name: name.clone(),
                    }),
                    issues,
                );
            }
        }
    }
}

fn collect_manage_py_candidates(
    db: &dyn Db,
    layout: &ProjectLayoutIndex,
    candidates: &mut Vec<SettingsCandidate>,
    issues: &mut Vec<SettingsCandidateIssue>,
) {
    for file in layout.files_by_name("manage.py") {
        let model = crate::python_source_model(db, file);
        if model.status() != &PythonSourceModelStatus::Parsed {
            continue;
        }
        for call in model.calls() {
            let Some(callee) = call.callee() else {
                continue;
            };
            if callee.as_dotted() != "os.environ.setdefault" {
                continue;
            }
            let [StaticValue::String(name), StaticValue::String(module), ..] = call.arguments()
            else {
                continue;
            };
            if name != "DJANGO_SETTINGS_MODULE" {
                continue;
            }
            push_value(
                candidates,
                module,
                Some(file),
                SettingsCandidateSource::ManagePyDefault,
                OriginSet::single(Origin::PythonSource { file }),
                issues,
            );
        }
    }
}

fn collect_conventional_candidates(
    db: &dyn Db,
    project: Project,
    layout: &ProjectLayoutIndex,
    candidates: &mut Vec<SettingsCandidate>,
) {
    for file in layout.files_by_name("settings.py") {
        let Some(path) = layout.file_path(file) else {
            continue;
        };
        let Some(module) = module_name_for_path(db, project, path) else {
            continue;
        };
        candidates.push(SettingsCandidate::new(
            module.clone(),
            Some(file),
            SettingsCandidateSource::ConventionalModule,
            OriginSet::single(Origin::Convention { file }),
        ));
    }
}

fn push_seed(
    candidates: &mut Vec<SettingsCandidate>,
    seed: &DjangoSettingsModuleSeed,
    file: Option<File>,
    source: SettingsCandidateSource,
    origin: OriginSet,
    issues: &mut Vec<SettingsCandidateIssue>,
) {
    push_value(candidates, seed.as_str(), file, source, origin, issues);
}

fn push_value(
    candidates: &mut Vec<SettingsCandidate>,
    value: &str,
    file: Option<File>,
    source: SettingsCandidateSource,
    origin: OriginSet,
    issues: &mut Vec<SettingsCandidateIssue>,
) {
    let Ok(module) = PyModuleName::parse(value) else {
        issues.push(SettingsCandidateIssue::InvalidModuleName {
            source,
            value: value.to_string(),
            origin,
        });
        return;
    };
    candidates.push(SettingsCandidate::new(module, file, source, origin));
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
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

    use super::*;
    use crate::DjangoEnvironmentSeed;
    use crate::DjangoSettingsModuleSeed;
    use crate::ProjectDiscoverySet;
    use crate::ProjectEnrichment;
    use crate::ProjectEnvVars;
    use crate::ProjectSourceFilesIssue;
    use crate::ProjectSourceInventory;
    use crate::ReadyProjectSourceFiles;
    use crate::RootDiscoveryInput;

    #[salsa::db]
    #[derive(Default)]
    struct TestDb {
        storage: salsa::Storage<Self>,
        files: SourceFiles,
        sources: FxHashMap<Utf8PathBuf, String>,
        project: OnceLock<Project>,
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
        let set = SourceFileSet::new(db, data);
        ProjectSourceInventory::Ready(ReadyProjectSourceFiles::merged_for_test(set))
    }

    fn sources_by_module(
        candidates: &[SettingsCandidate],
    ) -> BTreeSet<(String, SettingsCandidateSource)> {
        candidates
            .iter()
            .map(|candidate| {
                (
                    candidate.module().as_str().to_string(),
                    candidate.source().clone(),
                )
            })
            .collect()
    }

    #[test]
    fn settings_candidates_collect_explicit_env_manage_py_and_conventional_modules() {
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

        let SettingsCandidateOutcome::Ready { candidates, issues } =
            settings_candidates(&db, db.project());
        assert!(issues.is_empty());
        let sources = sources_by_module(candidates);

        assert!(sources.contains(&(
            "explicit.settings".to_string(),
            SettingsCandidateSource::ExplicitConfig,
        )));
        assert!(sources.contains(&(
            "environment.settings".to_string(),
            SettingsCandidateSource::ConfiguredEnvironment,
        )));
        assert!(sources.contains(&(
            "env.settings".to_string(),
            SettingsCandidateSource::EnvironmentVariable,
        )));
        assert!(sources.contains(&(
            "manage.settings".to_string(),
            SettingsCandidateSource::ManagePyDefault,
        )));
        assert!(sources.contains(&(
            "config.settings".to_string(),
            SettingsCandidateSource::ConventionalModule,
        )));
    }

    #[test]
    fn settings_candidates_strip_src_layout_prefix_for_conventional_modules() {
        let mut db = TestDb::with_project();
        db.set_file("/workspace/src/config/settings.py", "SECRET_KEY = 'x'\n");
        db.set_project_source_inventory(ready_inventory(
            &db,
            &["/workspace/src/config/settings.py"],
        ));

        let SettingsCandidateOutcome::Ready { candidates, issues } =
            settings_candidates(&db, db.project());
        let sources = sources_by_module(candidates);

        assert!(issues.is_empty());
        assert!(sources.contains(&(
            "config.settings".to_string(),
            SettingsCandidateSource::ConventionalModule,
        )));
    }

    #[test]
    fn settings_candidates_return_discovery_candidates_with_layout_issue() {
        let mut db = TestDb::with_project();
        let root = RootDiscoveryInput::new(
            &db,
            Utf8PathBuf::from("/workspace"),
            None,
            Some(DjangoSettingsModuleSeed::new("explicit.settings")),
            Vec::new(),
            Vec::new(),
            ProjectEnvVars::default(),
            Vec::new(),
        );
        db.set_project_discovery(ProjectDiscovery::Ready(
            ProjectDiscoverySet::new(vec![root]).expect("root should create discovery"),
        ));

        let SettingsCandidateOutcome::Ready { candidates, issues } =
            settings_candidates(&db, db.project());

        assert_eq!(candidates.len(), 1);
        assert!(matches!(
            issues.as_slice(),
            [SettingsCandidateIssue::LayoutUnavailable { .. }]
        ));
    }

    #[test]
    fn settings_candidates_report_invalid_module_values() {
        let mut db = TestDb::with_project();
        db.set_project_source_inventory(ready_inventory(&db, &[]));
        let root = RootDiscoveryInput::new(
            &db,
            Utf8PathBuf::from("/workspace"),
            None,
            Some(DjangoSettingsModuleSeed::new("not a module")),
            Vec::new(),
            Vec::new(),
            ProjectEnvVars::default(),
            Vec::new(),
        );
        db.set_project_discovery(ProjectDiscovery::Ready(
            ProjectDiscoverySet::new(vec![root]).expect("root should create discovery"),
        ));

        let SettingsCandidateOutcome::Ready { candidates, issues } =
            settings_candidates(&db, db.project());

        assert!(candidates.is_empty());
        assert!(matches!(
            issues.as_slice(),
            [SettingsCandidateIssue::InvalidModuleName {
                source: SettingsCandidateSource::ExplicitConfig,
                value,
                ..
            }] if value == "not a module"
        ));
    }
}

use djls_source::File;

use crate::layout::project_layout_index;
use crate::layout::ProjectLayoutIndex;
use crate::layout::ProjectLayoutIndexOutcome;
use crate::project::Project;
use crate::provenance::Origin;
use crate::provenance::OriginSet;
use crate::python::python_source_model;
use crate::python::PythonSourceParseStatus;
use crate::python::StaticValue;
use crate::resolver::module_name_for_path;
use crate::root_discovery::ProjectRootDiscovery;
use crate::Db;
use crate::PyModuleName;

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

#[salsa::tracked(returns(ref))]
pub fn settings_candidates(db: &dyn Db, project: Project) -> Vec<SettingsCandidate> {
    let mut candidates = discovery_candidates(db, project);

    if let ProjectLayoutIndexOutcome::Ready(layout) = project_layout_index(db, project) {
        candidates.extend(manage_py_candidates(db, layout));
        candidates.extend(conventional_candidates(db, project, layout));
    }

    candidates.sort_by(|left, right| {
        source_rank(&left.source)
            .cmp(&source_rank(&right.source))
            .then_with(|| left.module.as_str().cmp(right.module.as_str()))
    });

    candidates
}

fn discovery_candidates(db: &dyn Db, project: Project) -> Vec<SettingsCandidate> {
    let mut candidates = Vec::new();
    let ProjectRootDiscovery::Ready(discovery) = project.root_discovery(db) else {
        return candidates;
    };

    for root in discovery.roots() {
        if let Some(seed) = root.settings_module_seed(db) {
            candidates.extend(settings_candidate(
                seed.as_str(),
                None,
                SettingsCandidateSource::ExplicitConfig,
                OriginSet::single(Origin::Config {
                    root: root.root(db).clone(),
                }),
            ));
        }
        for environment in root.configured_environment_seeds(db) {
            let seed = environment.settings_module();
            candidates.extend(settings_candidate(
                seed.as_str(),
                None,
                SettingsCandidateSource::ConfiguredEnvironment,
                OriginSet::single(Origin::ConfiguredEnvironment {
                    root: environment.root().cloned(),
                    name: environment.name().map(str::to_string),
                }),
            ));
        }
        for (name, value) in root.env_vars(db).entries() {
            if name == "DJANGO_SETTINGS_MODULE" {
                candidates.extend(settings_candidate(
                    value,
                    None,
                    SettingsCandidateSource::EnvironmentVariable,
                    OriginSet::single(Origin::Environment {
                        root: root.root(db).clone(),
                        name: name.clone(),
                    }),
                ));
            }
        }
    }

    candidates
}

fn manage_py_candidates(db: &dyn Db, layout: &ProjectLayoutIndex) -> Vec<SettingsCandidate> {
    let mut candidates = Vec::new();
    for file in layout.files_by_name("manage.py") {
        let model = python_source_model(db, file);
        if model.parse_status() != &PythonSourceParseStatus::Parsed {
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
            candidates.extend(settings_candidate(
                module,
                Some(file),
                SettingsCandidateSource::ManagePyDefault,
                OriginSet::single(Origin::PythonSource { file }),
            ));
        }
    }
    candidates
}

fn conventional_candidates(
    db: &dyn Db,
    project: Project,
    layout: &ProjectLayoutIndex,
) -> Vec<SettingsCandidate> {
    let mut candidates = Vec::new();
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
    candidates
}

fn settings_candidate(
    value: &str,
    file: Option<File>,
    source: SettingsCandidateSource,
    origin: OriginSet,
) -> Option<SettingsCandidate> {
    Some(SettingsCandidate::new(
        PyModuleName::parse(value).ok()?,
        file,
        source,
        origin,
    ))
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
    use crate::enrichment::ProjectEnrichment;
    use crate::root_discovery::DjangoEnvironmentSeed;
    use crate::root_discovery::ProjectEnvVars;
    use crate::root_discovery::ProjectRootDiscoverySet;
    use crate::root_discovery::RootDiscoveryInput;
    use crate::source_files::ReadySourceFiles;
    use crate::source_files::SourceFileInventory;
    use crate::source_files::SourceFilesIssue;

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
                    SourceFileInventory::Unavailable {
                        issue: SourceFilesIssue::NotLoaded,
                    },
                    ProjectRootDiscovery::Absent,
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

    fn ready_inventory(db: &TestDb, paths: &[&str]) -> SourceFileInventory {
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
        SourceFileInventory::Ready(ReadySourceFiles::new(
            crate::source_files::SourceFileSetPartitions::default(),
            set,
        ))
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
        db.set_source_file_inventory(ready_inventory(
            &db,
            &["/workspace/manage.py", "/workspace/config/settings.py"],
        ));
        let root = RootDiscoveryInput::new(
            &db,
            Utf8PathBuf::from("/workspace"),
            None,
            Some("explicit.settings".to_string()),
            vec![DjangoEnvironmentSeed::from_settings_module(
                Some("default".to_string()),
                "environment.settings".to_string(),
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
        db.set_project_root_discovery(ProjectRootDiscovery::Ready(
            ProjectRootDiscoverySet::new(vec![root]).expect("root should create discovery"),
        ));

        let candidates = settings_candidates(&db, db.project());
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
        db.set_source_file_inventory(ready_inventory(&db, &["/workspace/src/config/settings.py"]));

        let candidates = settings_candidates(&db, db.project());
        let sources = sources_by_module(candidates);

        assert!(sources.contains(&(
            "config.settings".to_string(),
            SettingsCandidateSource::ConventionalModule,
        )));
    }

    #[test]
    fn settings_candidates_return_discovery_candidates_without_layout() {
        let mut db = TestDb::with_project();
        let root = RootDiscoveryInput::new(
            &db,
            Utf8PathBuf::from("/workspace"),
            None,
            Some("explicit.settings".to_string()),
            Vec::new(),
            Vec::new(),
            ProjectEnvVars::default(),
            Vec::new(),
        );
        db.set_project_root_discovery(ProjectRootDiscovery::Ready(
            ProjectRootDiscoverySet::new(vec![root]).expect("root should create discovery"),
        ));

        let candidates = settings_candidates(&db, db.project());

        assert_eq!(candidates.len(), 1);
    }

    #[test]
    fn settings_candidates_ignore_invalid_module_values() {
        let mut db = TestDb::with_project();
        db.set_source_file_inventory(ready_inventory(&db, &[]));
        let root = RootDiscoveryInput::new(
            &db,
            Utf8PathBuf::from("/workspace"),
            None,
            Some("not a module".to_string()),
            Vec::new(),
            Vec::new(),
            ProjectEnvVars::default(),
            Vec::new(),
        );
        db.set_project_root_discovery(ProjectRootDiscovery::Ready(
            ProjectRootDiscoverySet::new(vec![root]).expect("root should create discovery"),
        ));

        let candidates = settings_candidates(&db, db.project());

        assert!(candidates.is_empty());
    }
}

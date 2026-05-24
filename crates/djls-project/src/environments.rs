use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::File;

use crate::layout::project_layout_index;
use crate::layout::ProjectLayoutIndex;
use crate::layout::ProjectLayoutIndexOutcome;
use crate::project::Project;
use crate::python::python_source_model;
use crate::python::PythonSourceParseStatus;
use crate::python::StaticValue;
use crate::resolver::module_name_for_path;
use crate::root_discovery::ProjectRootDiscovery;
use crate::source_files::SourceFileInventory;
use crate::Db;
use crate::PyModuleName;

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
    settings: PyModuleName,
    root: Option<Utf8PathBuf>,
}

impl DjangoEnvironmentCandidate {
    #[must_use]
    pub fn id(&self) -> &DjangoEnvironmentId {
        &self.id
    }

    #[must_use]
    pub fn settings(&self) -> &PyModuleName {
        &self.settings
    }

    #[must_use]
    pub fn root(&self) -> Option<&Utf8Path> {
        self.root.as_deref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DjangoEnvironmentCandidatesOutcome {
    Ready(Vec<DjangoEnvironmentCandidate>),
    Deferred,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EnvironmentSelection {
    Selected(DjangoEnvironmentId),
    Ambiguous(Vec<DjangoEnvironmentCandidate>),
    Unknown,
}

#[salsa::tracked(returns(ref))]
pub fn django_environment_candidates(
    db: &dyn Db,
    project: Project,
) -> DjangoEnvironmentCandidatesOutcome {
    let mut candidates = Vec::new();
    add_discovery_environment_candidates(db, project, &mut candidates);

    if let ProjectLayoutIndexOutcome::Ready(layout) = project_layout_index(db, project) {
        add_manage_py_environment_candidates(db, project, layout, &mut candidates);
        add_conventional_environment_candidates(db, project, layout, &mut candidates);
    }

    if candidates.is_empty() {
        return DjangoEnvironmentCandidatesOutcome::Deferred;
    }

    DjangoEnvironmentCandidatesOutcome::Ready(candidates)
}

#[must_use]
pub(crate) fn known_django_environment_ids(
    db: &dyn Db,
    project: Project,
) -> Vec<DjangoEnvironmentId> {
    let DjangoEnvironmentCandidatesOutcome::Ready(candidates) =
        django_environment_candidates(db, project)
    else {
        return Vec::new();
    };
    candidates
        .iter()
        .map(|candidate| candidate.id().clone())
        .collect()
}

#[salsa::tracked(returns(ref))]
pub fn environment_for_file(db: &dyn Db, project: Project, file: File) -> EnvironmentSelection {
    let path = file.path(db);
    let candidates = match django_environment_candidates(db, project) {
        DjangoEnvironmentCandidatesOutcome::Ready(candidates) => candidates,
        DjangoEnvironmentCandidatesOutcome::Deferred => return EnvironmentSelection::Unknown,
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
        return EnvironmentSelection::Unknown;
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

    EnvironmentSelection::Ambiguous(matching)
}

fn add_discovery_environment_candidates(
    db: &dyn Db,
    project: Project,
    candidates: &mut Vec<DjangoEnvironmentCandidate>,
) {
    let ProjectRootDiscovery::Ready(roots) = project.root_discovery(db) else {
        return;
    };

    for root in roots {
        if let Some(seed) = root.settings_module_seed() {
            let Ok(settings) = PyModuleName::parse(seed.as_str()) else {
                continue;
            };
            add_environment_candidate(candidates, settings, Some(root.root().clone()));
        }
        for environment in root.configured_environment_seeds() {
            let seed = environment.settings_module();
            let Ok(settings) = PyModuleName::parse(seed.as_str()) else {
                continue;
            };
            add_environment_candidate(candidates, settings, environment.root().cloned());
        }
        for (name, value) in root.env_vars().entries() {
            if name == "DJANGO_SETTINGS_MODULE" {
                let Ok(settings) = PyModuleName::parse(value) else {
                    continue;
                };
                add_environment_candidate(candidates, settings, Some(root.root().clone()));
            }
        }
    }
}

fn add_manage_py_environment_candidates(
    db: &dyn Db,
    project: Project,
    layout: &ProjectLayoutIndex,
    candidates: &mut Vec<DjangoEnvironmentCandidate>,
) {
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
            let Ok(settings) = PyModuleName::parse(module) else {
                continue;
            };
            add_environment_candidate(
                candidates,
                settings,
                owning_root_for_path(db, project, file.path(db)),
            );
        }
    }
}

fn add_conventional_environment_candidates(
    db: &dyn Db,
    project: Project,
    layout: &ProjectLayoutIndex,
    candidates: &mut Vec<DjangoEnvironmentCandidate>,
) {
    for file in layout.files_by_name("settings.py") {
        let Some(path) = layout.file_path(file) else {
            continue;
        };
        let Some(settings) = module_name_for_path(db, project, path) else {
            continue;
        };
        add_environment_candidate(
            candidates,
            settings,
            owning_root_for_path(db, project, file.path(db)),
        );
    }
}

fn add_environment_candidate(
    candidates: &mut Vec<DjangoEnvironmentCandidate>,
    settings: PyModuleName,
    root: Option<Utf8PathBuf>,
) {
    if candidates
        .iter()
        .any(|candidate| candidate.settings == settings && candidate.root == root)
    {
        return;
    }

    candidates.push(DjangoEnvironmentCandidate {
        id: DjangoEnvironmentId(format!(
            "{}:{}",
            settings.as_str(),
            root.as_ref().map_or("", |root| root.as_str()),
        )),
        settings,
        root,
    });
}

fn owning_root_for_path(db: &dyn Db, project: Project, path: &Utf8Path) -> Option<Utf8PathBuf> {
    let mut roots = Vec::new();
    if let SourceFileInventory::Ready(files) = project.source_inventory(db) {
        roots.extend(
            files
                .merged()
                .data(db)
                .roots()
                .iter()
                .map(|entry| entry.root().path().to_owned()),
        );
    }
    if let ProjectRootDiscovery::Ready(discovery_roots) = project.root_discovery(db) {
        roots.extend(discovery_roots.iter().map(|root| root.root().clone()));
    }
    roots
        .into_iter()
        .filter(|root| path.starts_with(root.as_path()))
        .max_by_key(|root| root.as_str().len())
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
    use crate::enrichment::ProjectEnrichment;
    use crate::root_discovery::DjangoEnvironmentSeed;
    use crate::root_discovery::ProjectEnvVars;
    use crate::root_discovery::ProjectRoot;
    use crate::root_discovery::ProjectRootDiscovery;
    use crate::source_files::ReadySourceFiles;
    use crate::source_files::SourceFileInventory;
    use crate::source_files::SourceFilesIssue;

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
                        .push(event);
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
        SourceFileInventory::Ready(ReadySourceFiles::new(
            crate::source_files::SourceFileSetPartitions::default(),
            SourceFileSet::new(db, data),
        ))
    }

    fn discovery_root(
        _db: &TestDb,
        root: &str,
        settings: Option<&str>,
        environments: Vec<DjangoEnvironmentSeed>,
    ) -> ProjectRoot {
        ProjectRoot::new(
            Utf8PathBuf::from(root),
            None,
            settings.map(str::to_string),
            environments,
            Vec::new(),
            ProjectEnvVars::default(),
            Vec::new(),
        )
    }

    #[test]
    fn environments_create_candidates_from_every_settings_source() {
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
        let root = ProjectRoot::new(
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
        db.set_project_root_discovery(ProjectRootDiscovery::Ready(vec![root]));

        let DjangoEnvironmentCandidatesOutcome::Ready(candidates) =
            django_environment_candidates(&db, db.project())
        else {
            panic!("multiple discoverable candidates should be ready");
        };
        let settings = candidates
            .iter()
            .map(|candidate| candidate.settings().as_str().to_string())
            .collect::<std::collections::BTreeSet<_>>();

        assert_eq!(settings.len(), 5);
        assert!(settings.contains("explicit.settings"));
        assert!(settings.contains("environment.settings"));
        assert!(settings.contains("env.settings"));
        assert!(settings.contains("manage.settings"));
        assert!(settings.contains("config.settings"));
    }

    #[test]
    fn environments_deduplicate_same_settings_and_root() {
        let mut db = TestDb::with_project();
        db.set_source_file_inventory(ready_inventory(&db, &[]));
        let root = ProjectRoot::new(
            Utf8PathBuf::from("/workspace"),
            None,
            Some("project.settings".to_string()),
            Vec::new(),
            Vec::new(),
            ProjectEnvVars::from_resolved_entries(vec![(
                "DJANGO_SETTINGS_MODULE".to_string(),
                "project.settings".to_string(),
            )])
            .expect("env vars should be valid"),
            Vec::new(),
        );
        db.set_project_root_discovery(ProjectRootDiscovery::Ready(vec![root]));

        let DjangoEnvironmentCandidatesOutcome::Ready(candidates) =
            django_environment_candidates(&db, db.project())
        else {
            panic!("valid candidates should be ready");
        };

        assert_eq!(candidates.len(), 1);
    }

    #[test]
    fn environments_ignore_invalid_settings_candidate_values_with_valid_candidates() {
        let mut db = TestDb::with_project();
        db.set_source_file_inventory(ready_inventory(&db, &[]));
        let root = ProjectRoot::new(
            Utf8PathBuf::from("/workspace"),
            None,
            Some("explicit.settings".to_string()),
            Vec::new(),
            Vec::new(),
            ProjectEnvVars::from_resolved_entries(vec![(
                "DJANGO_SETTINGS_MODULE".to_string(),
                "not a module".to_string(),
            )])
            .expect("env vars should be valid"),
            Vec::new(),
        );
        db.set_project_root_discovery(ProjectRootDiscovery::Ready(vec![root]));

        let DjangoEnvironmentCandidatesOutcome::Ready(candidates) =
            django_environment_candidates(&db, db.project())
        else {
            panic!("valid candidates should remain ready");
        };

        assert_eq!(candidates.len(), 1);
    }

    #[test]
    fn conventional_environment_root_is_project_root_not_settings_package() {
        let mut db = TestDb::with_project();
        let file = db.set_file("/workspace/templates/index.html", "hi");
        db.set_file("/workspace/config/settings.py", "SECRET_KEY = 'x'\n");
        db.set_source_file_inventory(ready_inventory(
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
        db.set_source_file_inventory(ready_inventory(&db, &[]));
        db.set_project_root_discovery(ProjectRootDiscovery::Ready(vec![root]));

        let DjangoEnvironmentCandidatesOutcome::Ready(candidates) =
            django_environment_candidates(&db, db.project())
        else {
            panic!("single environment candidate should be ready");
        };
        assert_eq!(candidates.len(), 1);
        let _ = db.take_events();

        let DjangoEnvironmentCandidatesOutcome::Ready(candidates) =
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
        db.set_source_file_inventory(ready_inventory(
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
        db.set_project_root_discovery(ProjectRootDiscovery::Ready(vec![site_a, site_b]));

        let EnvironmentSelection::Selected(selected) =
            environment_for_file(&db, db.project(), file)
        else {
            panic!("site_b template should select site_b environment");
        };

        assert!(selected.as_str().contains("site_b.settings"));
    }
}

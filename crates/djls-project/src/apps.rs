use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::File;
use djls_source::FileRootKind;
use djls_workspace::FilesForRootsRequest;
use djls_workspace::FilesForRootsResult;
use djls_workspace::WalkOptions;

use crate::build_source_roots_with_kind;
use crate::django_environment_candidates;
use crate::effective_settings;
use crate::resolve_module;
use crate::Db;
use crate::DjangoEnvironmentCandidatesOutcome;
use crate::DjangoEnvironmentId;
use crate::ModuleResolutionOutcome;
use crate::PartitionedSourceFileLoadOutcome;
use crate::PartitionedSourceFilePatch;
use crate::Project;
use crate::ProjectSourceFilesIssue;
use crate::PyModuleName;
use crate::SettingsIssue;
use crate::StaticValue;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstalledApp {
    entry: String,
    resolution: InstalledAppResolution,
}

impl InstalledApp {
    #[must_use]
    pub fn entry(&self) -> &str {
        &self.entry
    }

    #[must_use]
    pub fn resolution(&self) -> &InstalledAppResolution {
        &self.resolution
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppConfig {
    module: PyModuleName,
    name: Option<String>,
    label: Option<String>,
    path: Option<Utf8PathBuf>,
}

impl AppConfig {
    #[must_use]
    pub fn module(&self) -> &PyModuleName {
        &self.module
    }

    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    #[must_use]
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    #[must_use]
    pub fn path(&self) -> Option<&camino::Utf8Path> {
        self.path.as_deref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InstalledAppResolution {
    Package { module: PyModuleName, file: File },
    AppConfig { config: AppConfig, file: File },
    Missing { issue: InstalledAppIssue },
    Ambiguous { issue: InstalledAppIssue },
    Deferred { issue: InstalledAppIssue },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InstalledAppIssue {
    UnknownInstalledAppSegment { issue: SettingsIssue },
    InvalidModuleName { value: String },
    ModuleNotFound { module: PyModuleName },
    ModuleAmbiguous { module: PyModuleName },
    ModuleDeferred { module: PyModuleName },
    AppConfigDetailsDeferred { module: PyModuleName },
}

#[salsa::tracked(returns(ref))]
pub fn installed_apps(
    db: &dyn Db,
    project: Project,
    env: DjangoEnvironmentId,
) -> Vec<InstalledApp> {
    effective_settings(db, project, env)
        .installed_apps()
        .segments()
        .iter()
        .map(|segment| match segment.value() {
            Some(entry) => InstalledApp {
                entry: entry.clone(),
                resolution: resolve_installed_app_entry(db, project, entry),
            },
            None => InstalledApp {
                entry: String::new(),
                resolution: InstalledAppResolution::Missing {
                    issue: InstalledAppIssue::UnknownInstalledAppSegment {
                        issue: segment.issue().cloned().unwrap_or(
                            SettingsIssue::UnsupportedListOperation {
                                operation: "unknown-installed-app-segment",
                            },
                        ),
                    },
                },
            },
        })
        .collect()
}

fn resolve_installed_app_entry(
    db: &dyn Db,
    project: Project,
    entry: &str,
) -> InstalledAppResolution {
    if let Some((module, class_name)) = split_app_config_entry(entry) {
        return resolve_app_config(db, project, module, class_name);
    }

    let Ok(module) = PyModuleName::parse(entry) else {
        return InstalledAppResolution::Missing {
            issue: InstalledAppIssue::InvalidModuleName {
                value: entry.to_string(),
            },
        };
    };

    match resolve_module(db, project, module.clone()).outcome() {
        ModuleResolutionOutcome::Resolved(resolved) => InstalledAppResolution::Package {
            module,
            file: resolved.location().file(),
        },
        ModuleResolutionOutcome::NotFound { .. } => InstalledAppResolution::Missing {
            issue: InstalledAppIssue::ModuleNotFound { module },
        },
        ModuleResolutionOutcome::Ambiguous { .. } => InstalledAppResolution::Ambiguous {
            issue: InstalledAppIssue::ModuleAmbiguous { module },
        },
        ModuleResolutionOutcome::Deferred { .. } => InstalledAppResolution::Deferred {
            issue: InstalledAppIssue::ModuleDeferred { module },
        },
    }
}

fn resolve_app_config(
    db: &dyn Db,
    project: Project,
    module: PyModuleName,
    class_name: &str,
) -> InstalledAppResolution {
    match resolve_module(db, project, module.clone()).outcome() {
        ModuleResolutionOutcome::Resolved(resolved) => {
            let file = resolved.location().file();
            let model = crate::python_source_model(db, file);
            InstalledAppResolution::AppConfig {
                config: AppConfig {
                    module,
                    name: static_app_config_string_assignment(
                        model.class_defs(),
                        class_name,
                        "name",
                    ),
                    label: static_app_config_string_assignment(
                        model.class_defs(),
                        class_name,
                        "label",
                    ),
                    path: static_app_config_string_assignment(
                        model.class_defs(),
                        class_name,
                        "path",
                    )
                    .map(Utf8PathBuf::from),
                },
                file,
            }
        }
        ModuleResolutionOutcome::NotFound { .. } => InstalledAppResolution::Missing {
            issue: InstalledAppIssue::ModuleNotFound { module },
        },
        ModuleResolutionOutcome::Ambiguous { .. } => InstalledAppResolution::Ambiguous {
            issue: InstalledAppIssue::ModuleAmbiguous { module },
        },
        ModuleResolutionOutcome::Deferred { .. } => InstalledAppResolution::Deferred {
            issue: InstalledAppIssue::AppConfigDetailsDeferred { module },
        },
    }
}

fn split_app_config_entry(entry: &str) -> Option<(PyModuleName, &str)> {
    let (module, class_name) = entry.rsplit_once('.')?;
    let last = class_name.chars().next()?;
    if !last.is_uppercase() {
        return None;
    }
    Some((PyModuleName::parse(module).ok()?, class_name))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstalledAppFilesLoadRequest {
    roots: Vec<Utf8PathBuf>,
}

impl InstalledAppFilesLoadRequest {
    #[must_use]
    pub fn new(roots: Vec<Utf8PathBuf>) -> Self {
        Self { roots }
    }

    #[must_use]
    pub fn roots(&self) -> &[Utf8PathBuf] {
        &self.roots
    }
}

#[must_use]
pub fn installed_app_files_request(request: InstalledAppFilesLoadRequest) -> FilesForRootsRequest {
    let plan = build_source_roots_with_kind(request.roots, FileRootKind::LibrarySearchPath);
    FilesForRootsRequest::new(
        plan.roots().to_vec(),
        Box::new(installed_app_file_predicate),
        django_app_walk_options(),
    )
}

#[must_use]
pub fn load_installed_app_files(request: InstalledAppFilesLoadRequest) -> FilesForRootsResult {
    djls_workspace::load_files_for_roots(installed_app_files_request(request))
}

#[must_use]
pub fn installed_app_file_roots(db: &dyn Db, project: Project) -> InstalledAppFilesLoadRequest {
    let (roots, _) = installed_app_file_roots_and_issues(db, project);
    InstalledAppFilesLoadRequest::new(roots)
}

#[must_use]
pub fn installed_app_file_load_outcome(
    db: &dyn Db,
    project: Project,
) -> PartitionedSourceFileLoadOutcome {
    match django_environment_candidates(db, project) {
        DjangoEnvironmentCandidatesOutcome::Deferred { .. } => {
            return PartitionedSourceFileLoadOutcome::Deferred {
                issue: ProjectSourceFilesIssue::InstalledAppGap {
                    entry: "<environment-discovery>".to_string(),
                },
            };
        }
        DjangoEnvironmentCandidatesOutcome::Unavailable { .. } => {
            return PartitionedSourceFileLoadOutcome::Unavailable {
                issue: ProjectSourceFilesIssue::InstalledAppGap {
                    entry: "<environment-discovery>".to_string(),
                },
            };
        }
        DjangoEnvironmentCandidatesOutcome::Ready { .. }
        | DjangoEnvironmentCandidatesOutcome::Ambiguous { .. } => {}
    }
    let (roots, issues) = installed_app_file_roots_and_issues(db, project);
    let result = load_installed_app_files(InstalledAppFilesLoadRequest::new(roots));
    let patches = PartitionedSourceFilePatch::installed_app(result);
    if issues.is_empty() {
        PartitionedSourceFileLoadOutcome::Ready(patches)
    } else {
        PartitionedSourceFileLoadOutcome::Degraded { patches, issues }
    }
}

fn installed_app_file_roots_and_issues(
    db: &dyn Db,
    project: Project,
) -> (Vec<Utf8PathBuf>, Vec<ProjectSourceFilesIssue>) {
    let mut roots = Vec::new();
    let mut issues = Vec::new();
    let candidates = match django_environment_candidates(db, project) {
        DjangoEnvironmentCandidatesOutcome::Ready { candidates, .. }
        | DjangoEnvironmentCandidatesOutcome::Ambiguous { candidates, .. } => candidates,
        DjangoEnvironmentCandidatesOutcome::Unavailable { .. }
        | DjangoEnvironmentCandidatesOutcome::Deferred { .. } => return (roots, issues),
    };

    for candidate in candidates {
        for app in installed_apps(db, project, candidate.id().clone()) {
            match app.resolution() {
                InstalledAppResolution::Package { file, .. } => {
                    if let Some(root) = app_root_for_file(db, *file) {
                        roots.push(root);
                    }
                }
                InstalledAppResolution::AppConfig { config, file } => {
                    if let Some(path) = config.path() {
                        roots.push(path.to_owned());
                    } else if let Some(root) = app_root_for_file(db, *file) {
                        roots.push(root);
                    }
                }
                InstalledAppResolution::Missing { .. }
                | InstalledAppResolution::Ambiguous { .. }
                | InstalledAppResolution::Deferred { .. } => {
                    issues.push(ProjectSourceFilesIssue::InstalledAppGap {
                        entry: app.entry().to_string(),
                    });
                }
            }
        }
    }
    roots.sort();
    roots.dedup();
    (roots, issues)
}

fn app_root_for_file(db: &dyn Db, file: File) -> Option<Utf8PathBuf> {
    let path = file.path(db);
    let parent = path.parent()?;
    if path.file_name() == Some("__init__.py") || path.file_name() == Some("apps.py") {
        return Some(parent.to_owned());
    }
    parent.parent().map(Utf8Path::to_owned)
}

fn django_app_walk_options() -> WalkOptions {
    WalkOptions {
        hidden: false,
        globs: vec!["!**/__pycache__/**".to_string()],
        no_ignore: false,
        follow_links: false,
        max_depth: None,
    }
}

fn installed_app_file_predicate(path: &Utf8Path) -> bool {
    if matches!(
        path.file_name(),
        Some("apps.py" | "models.py" | "admin.py" | "urls.py" | "forms.py")
    ) {
        return true;
    }

    path.components()
        .any(|component| matches!(component.as_str(), "models" | "templates" | "templatetags"))
}

fn static_app_config_string_assignment(
    classes: &[crate::ClassDef],
    class_name: &str,
    name: &str,
) -> Option<String> {
    let class = classes.iter().find(|class| class.name() == class_name)?;
    static_string_assignment(class.assignments(), name)
}

fn static_string_assignment(assignments: &[crate::Assignment], name: &str) -> Option<String> {
    assignments.iter().find_map(|assignment| {
        let matches_name = assignment
            .targets()
            .iter()
            .any(|target| target.name().as_dotted() == name);
        if !matches_name {
            return None;
        }
        match assignment.value() {
            StaticValue::String(value) => Some(value.clone()),
            _ => None,
        }
    })
}

#[cfg(test)]
mod tests {
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
    use crate::django_environment_candidates;
    use crate::DjangoEnvironmentCandidatesOutcome;
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
        ProjectSourceInventory::Ready(ReadyProjectSourceFiles::merged_for_test(
            SourceFileSet::new(db, data),
        ))
    }

    fn discovery(db: &TestDb) -> ProjectDiscovery {
        let root = RootDiscoveryInput::new(
            db,
            Utf8PathBuf::from("/workspace"),
            None,
            Some(DjangoSettingsModuleSeed::new("project.settings")),
            Vec::new(),
            Vec::new(),
            ProjectEnvVars::default(),
            Vec::new(),
        );
        ProjectDiscovery::Ready(
            ProjectDiscoverySet::new(vec![root]).expect("root should create discovery"),
        )
    }

    fn single_env_id(db: &TestDb) -> DjangoEnvironmentId {
        let DjangoEnvironmentCandidatesOutcome::Ready { candidates, .. } =
            django_environment_candidates(db, db.project())
        else {
            panic!("single candidate should be ready");
        };
        candidates[0].id().clone()
    }

    #[test]
    fn installed_app_files_loads_django_relevant_files_only() {
        let tempdir = tempfile::tempdir().expect("tempdir should be created");
        let root = utf8(tempdir.path()).join("blog");
        std::fs::create_dir_all(root.join("templates/blog"))
            .expect("templates directory should be created");
        std::fs::create_dir_all(root.join("templatetags"))
            .expect("templatetags directory should be created");
        std::fs::create_dir_all(root.join("migrations"))
            .expect("migrations directory should be created");
        std::fs::write(root.join("apps.py"), "").expect("apps.py should be written");
        std::fs::write(root.join("models.py"), "").expect("models.py should be written");
        std::fs::write(root.join("templates/blog/index.html"), "")
            .expect("template should be written");
        std::fs::write(root.join("templatetags/blog_tags.py"), "")
            .expect("tag library should be written");
        std::fs::write(root.join("migrations/0001_initial.py"), "")
            .expect("migration should be written");

        let result = load_installed_app_files(InstalledAppFilesLoadRequest::new(vec![root]));
        let loaded = result
            .files()
            .iter()
            .map(|file| file.path().file_name().unwrap().to_string())
            .collect::<Vec<_>>();

        assert!(loaded.contains(&"apps.py".to_string()));
        assert!(loaded.contains(&"models.py".to_string()));
        assert!(loaded.contains(&"index.html".to_string()));
        assert!(loaded.contains(&"blog_tags.py".to_string()));
        assert!(!loaded.contains(&"0001_initial.py".to_string()));
        assert_eq!(result.roots()[0].kind(), FileRootKind::LibrarySearchPath);
    }

    fn utf8(path: &std::path::Path) -> Utf8PathBuf {
        Utf8PathBuf::from_path_buf(path.to_path_buf()).expect("path should be utf8")
    }

    #[test]
    fn installed_apps_resolve_packages_and_preserve_order() {
        let mut db = TestDb::with_project();
        db.set_file(
            "/workspace/project/settings.py",
            "INSTALLED_APPS = ['django.contrib.auth', 'blog']\n",
        );
        db.set_file("/workspace/django/contrib/auth/__init__.py", "");
        db.set_file("/workspace/blog/__init__.py", "");
        db.set_project_source_inventory(ready_inventory(
            &db,
            &[
                "/workspace/project/settings.py",
                "/workspace/django/contrib/auth/__init__.py",
                "/workspace/blog/__init__.py",
            ],
        ));
        db.set_project_discovery(discovery(&db));
        let env = single_env_id(&db);

        let apps = installed_apps(&db, db.project(), env);

        assert_eq!(apps[0].entry(), "django.contrib.auth");
        assert_eq!(apps[1].entry(), "blog");
        assert!(matches!(
            apps[0].resolution(),
            InstalledAppResolution::Package { .. }
        ));
    }

    #[test]
    fn installed_apps_preserve_unknown_segment_issue() {
        let mut db = TestDb::with_project();
        db.set_file(
            "/workspace/project/settings.py",
            "INSTALLED_APPS = ['known', UNKNOWN]\n",
        );
        db.set_file("/workspace/known/__init__.py", "");
        db.set_project_source_inventory(ready_inventory(
            &db,
            &[
                "/workspace/project/settings.py",
                "/workspace/known/__init__.py",
            ],
        ));
        db.set_project_discovery(discovery(&db));
        let env = single_env_id(&db);

        let apps = installed_apps(&db, db.project(), env);

        assert!(matches!(
            apps[1].resolution(),
            InstalledAppResolution::Missing {
                issue: InstalledAppIssue::UnknownInstalledAppSegment { .. },
            }
        ));
    }

    #[test]
    fn installed_apps_resolve_static_app_config_details() {
        let mut db = TestDb::with_project();
        db.set_file(
            "/workspace/project/settings.py",
            "INSTALLED_APPS = ['blog.apps.BlogConfig']\n",
        );
        db.set_file(
            "/workspace/blog/apps.py",
            "from django.apps import AppConfig\nclass OtherConfig(AppConfig):\n    name = 'wrong'\n    label = 'wrong'\nclass BlogConfig(AppConfig):\n    name = 'blog'\n    label = 'weblog'\n    path = '/srv/blog'\n",
        );
        db.set_project_source_inventory(ready_inventory(
            &db,
            &["/workspace/project/settings.py", "/workspace/blog/apps.py"],
        ));
        db.set_project_discovery(discovery(&db));
        let env = single_env_id(&db);

        let apps = installed_apps(&db, db.project(), env);

        let InstalledAppResolution::AppConfig { config, .. } = apps[0].resolution() else {
            panic!("app config should resolve");
        };
        assert_eq!(config.name(), Some("blog"));
        assert_eq!(config.label(), Some("weblog"));
        assert_eq!(
            config.path().map(camino::Utf8Path::as_str),
            Some("/srv/blog")
        );
    }

    #[test]
    fn installed_apps_defer_app_config_details_when_module_root_is_not_loaded() {
        let mut db = TestDb::with_project();
        db.set_file(
            "/workspace/project/settings.py",
            "INSTALLED_APPS = ['external.apps.ExternalConfig']\n",
        );
        db.set_project_source_inventory(ready_inventory(&db, &["/workspace/project/settings.py"]));
        let root = RootDiscoveryInput::new(
            &db,
            Utf8PathBuf::from("/workspace"),
            None,
            Some(DjangoSettingsModuleSeed::new("project.settings")),
            Vec::new(),
            vec![Utf8PathBuf::from("/external")],
            ProjectEnvVars::default(),
            Vec::new(),
        );
        db.set_project_discovery(ProjectDiscovery::Ready(
            ProjectDiscoverySet::new(vec![root]).expect("root should create discovery"),
        ));
        let env = single_env_id(&db);

        let apps = installed_apps(&db, db.project(), env);

        assert!(matches!(
            apps[0].resolution(),
            InstalledAppResolution::Deferred {
                issue: InstalledAppIssue::AppConfigDetailsDeferred { .. },
            }
        ));
    }
}

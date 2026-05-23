use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::File;
use djls_source::FileRootKind;
use djls_workspace::FilesForRootsRequest;
use djls_workspace::FilesForRootsResult;
use djls_workspace::WalkOptions;

use crate::django_environment_candidates;
use crate::project::Project;
use crate::python::python_source_model;
use crate::python::Assignment;
use crate::python::ClassDef;
use crate::python::StaticValue;
use crate::resolver::resolve_module;
use crate::resolver::ModuleResolutionOutcome;
use crate::settings::django_settings;
use crate::settings::PartialListSegment;
use crate::source_files::build_source_roots_with_kind;
use crate::source_files::merge_partitioned_source_file_patch_set;
use crate::source_files::PartitionedSourceFilePatchSet;
use crate::source_files::ReadySourceFiles;
use crate::source_files::SourceFilesIssue;
use crate::source_files::SourceFilesUpdate;
use crate::Db;
use crate::DjangoEnvironmentCandidatesOutcome;
use crate::DjangoEnvironmentId;
use crate::PyModuleName;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InstalledApp {
    entry: String,
    module: PyModuleName,
    file: File,
    config: Option<AppConfig>,
}

impl InstalledApp {
    #[must_use]
    pub(crate) fn entry(&self) -> &str {
        &self.entry
    }

    #[must_use]
    pub(crate) fn module(&self) -> &PyModuleName {
        &self.module
    }

    #[must_use]
    pub(crate) fn file(&self) -> File {
        self.file
    }

    #[must_use]
    pub(crate) fn config(&self) -> Option<&AppConfig> {
        self.config.as_ref()
    }

    #[must_use]
    pub(crate) fn root(&self, db: &dyn Db) -> Option<Utf8PathBuf> {
        self.config()
            .and_then(AppConfig::path)
            .map(Utf8Path::to_owned)
            .or_else(|| app_root_for_file(db, self.file()))
    }

    #[must_use]
    pub(crate) fn template_dir(&self, db: &dyn Db) -> Option<Utf8PathBuf> {
        Some(self.root(db)?.join("templates"))
    }

    #[must_use]
    pub(crate) fn module_name_for_path(
        &self,
        db: &dyn Db,
        path: &Utf8Path,
    ) -> Option<PyModuleName> {
        let root = self.root(db)?;
        if !path.starts_with(root.as_path()) {
            return None;
        }
        let relative = path.strip_prefix(root.as_path()).ok()?.with_extension("");
        let relative = relative
            .components()
            .map(|component| component.as_str())
            .collect::<Vec<_>>()
            .join(".");
        let module = if relative.is_empty() || relative == "__init__" {
            self.module().as_str().to_string()
        } else {
            format!("{}.{}", self.module().as_str(), relative)
        };
        PyModuleName::parse(&module).ok()
    }

    fn from_settings_segment(
        db: &dyn Db,
        project: Project,
        segment: &PartialListSegment<String>,
    ) -> Option<Self> {
        Self::resolve_entry(db, project, segment.value()?)
    }

    fn resolve_entry(db: &dyn Db, project: Project, entry: &str) -> Option<Self> {
        if let Some((module, class_name)) = Self::split_app_config_entry(entry) {
            return Self::resolve_app_config(db, project, entry, module, class_name);
        }

        let module = PyModuleName::parse(entry).ok()?;
        let resolved = match resolve_module(db, project, module.clone()).outcome() {
            ModuleResolutionOutcome::Resolved(resolved) => resolved,
            ModuleResolutionOutcome::Unresolved(_) => return None,
        };
        Some(Self {
            entry: entry.to_string(),
            module,
            file: resolved.location().file(),
            config: None,
        })
    }

    fn resolve_app_config(
        db: &dyn Db,
        project: Project,
        entry: &str,
        config_module: PyModuleName,
        class_name: &str,
    ) -> Option<Self> {
        let resolved = match resolve_module(db, project, config_module.clone()).outcome() {
            ModuleResolutionOutcome::Resolved(resolved) => resolved,
            ModuleResolutionOutcome::Unresolved(_) => return None,
        };
        let file = resolved.location().file();
        let model = python_source_model(db, file);
        let config = AppConfig::from_class_defs(config_module, model.class_defs(), class_name);
        let module = config
            .name()
            .and_then(|name| PyModuleName::parse(name).ok())
            .or_else(|| config.module().parent())?;
        Some(Self {
            entry: entry.to_string(),
            module,
            file,
            config: Some(config),
        })
    }

    fn split_app_config_entry(entry: &str) -> Option<(PyModuleName, &str)> {
        let (module, class_name) = entry.rsplit_once('.')?;
        let last = class_name.chars().next()?;
        if !last.is_uppercase() {
            return None;
        }
        Some((PyModuleName::parse(module).ok()?, class_name))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AppConfig {
    module: PyModuleName,
    name: Option<String>,
    label: Option<String>,
    path: Option<Utf8PathBuf>,
}

impl AppConfig {
    fn from_class_defs(module: PyModuleName, classes: &[ClassDef], class_name: &str) -> Self {
        let class = classes.iter().find(|class| class.name() == class_name);
        Self {
            module,
            name: class.and_then(|class| Self::string_assignment(class.assignments(), "name")),
            label: class.and_then(|class| Self::string_assignment(class.assignments(), "label")),
            path: class
                .and_then(|class| Self::string_assignment(class.assignments(), "path"))
                .map(Utf8PathBuf::from),
        }
    }

    #[must_use]
    pub(crate) fn module(&self) -> &PyModuleName {
        &self.module
    }

    #[must_use]
    pub(crate) fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    #[must_use]
    pub(crate) fn path(&self) -> Option<&Utf8Path> {
        self.path.as_deref()
    }

    fn string_assignment(assignments: &[Assignment], name: &str) -> Option<String> {
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
}

#[salsa::tracked(returns(ref))]
pub(crate) fn installed_apps(
    db: &dyn Db,
    project: Project,
    env: DjangoEnvironmentId,
) -> Vec<InstalledApp> {
    django_settings(db, project, env)
        .installed_app_entries()
        .segments()
        .iter()
        .filter_map(|segment| InstalledApp::from_settings_segment(db, project, segment))
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InstalledAppFileRootsOutcome {
    Ready(InstalledAppFileRoots),
    Deferred,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstalledAppFileRoots {
    roots: Vec<Utf8PathBuf>,
    issues: Vec<SourceFilesIssue>,
}

impl InstalledAppFileRoots {
    pub(crate) fn new(roots: Vec<Utf8PathBuf>, issues: Vec<SourceFilesIssue>) -> Self {
        Self { roots, issues }
    }

    #[must_use]
    pub fn roots(&self) -> &[Utf8PathBuf] {
        &self.roots
    }

    #[must_use]
    pub fn issues(&self) -> &[SourceFilesIssue] {
        &self.issues
    }

    pub(crate) fn files_request(&self) -> FilesForRootsRequest {
        let plan =
            build_source_roots_with_kind(self.roots.clone(), FileRootKind::LibrarySearchPath);
        FilesForRootsRequest::new(
            plan.roots().to_vec(),
            Box::new(|path| {
                matches!(
                    path.file_name(),
                    Some("apps.py" | "models.py" | "admin.py" | "urls.py" | "forms.py")
                ) || path.components().any(|component| {
                    matches!(component.as_str(), "models" | "templates" | "templatetags")
                })
            }),
            WalkOptions {
                hidden: false,
                globs: vec!["!**/__pycache__/**".to_string()],
                no_ignore: false,
                follow_links: false,
                max_depth: None,
            },
        )
    }

    pub(crate) fn source_files_update(
        &self,
        current: Option<&ReadySourceFiles>,
        result: FilesForRootsResult,
    ) -> SourceFilesUpdate {
        merge_partitioned_source_file_patch_set(
            current,
            PartitionedSourceFilePatchSet::installed_apps(result, self.issues.clone()),
        )
    }
}

#[must_use]
pub fn installed_app_file_roots_discovery(
    db: &dyn Db,
    project: Project,
) -> InstalledAppFileRootsOutcome {
    let candidates = match django_environment_candidates(db, project) {
        DjangoEnvironmentCandidatesOutcome::Ready(candidates) => candidates,
        DjangoEnvironmentCandidatesOutcome::Deferred => {
            return InstalledAppFileRootsOutcome::Deferred
        }
    };
    let mut roots = Vec::new();
    let mut issues = Vec::new();

    for candidate in candidates {
        for segment in django_settings(db, project, candidate.id().clone())
            .installed_app_entries()
            .segments()
        {
            match InstalledApp::from_settings_segment(db, project, segment) {
                Some(app) => {
                    if let Some(root) = app.root(db) {
                        roots.push(root);
                    }
                }
                None => issues.push(SourceFilesIssue::InstalledAppGap),
            }
        }
    }
    roots.sort();
    roots.dedup();
    InstalledAppFileRootsOutcome::Ready(InstalledAppFileRoots::new(roots, issues))
}

fn app_root_for_file(db: &dyn Db, file: File) -> Option<Utf8PathBuf> {
    let path = file.path(db);
    let parent = path.parent()?;
    if path.file_name() == Some("__init__.py") || path.file_name() == Some("apps.py") {
        return Some(parent.to_owned());
    }
    parent.parent().map(Utf8Path::to_owned)
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
    use crate::enrichment::ProjectEnrichment;
    use crate::root_discovery::ProjectEnvVars;
    use crate::root_discovery::ProjectRootDiscovery;
    use crate::root_discovery::ProjectRootDiscoverySet;
    use crate::root_discovery::RootDiscoveryInput;
    use crate::source_files::ReadySourceFiles;
    use crate::source_files::SourceFileInventory;
    use crate::source_files::SourceFilesIssue;
    use crate::DjangoEnvironmentCandidatesOutcome;

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
        SourceFileInventory::Ready(ReadySourceFiles::new(
            crate::source_files::SourceFileSetPartitions::default(),
            SourceFileSet::new(db, data),
        ))
    }

    fn discovery(db: &TestDb) -> ProjectRootDiscovery {
        let root = RootDiscoveryInput::new(
            db,
            Utf8PathBuf::from("/workspace"),
            None,
            Some("project.settings".to_string()),
            Vec::new(),
            Vec::new(),
            ProjectEnvVars::default(),
            Vec::new(),
        );
        ProjectRootDiscovery::Ready(
            ProjectRootDiscoverySet::new(vec![root]).expect("root should create discovery"),
        )
    }

    fn single_env_id(db: &TestDb) -> DjangoEnvironmentId {
        let DjangoEnvironmentCandidatesOutcome::Ready(candidates) =
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

        let result = djls_workspace::load_files_for_roots(
            InstalledAppFileRoots::new(vec![root], Vec::new()).files_request(),
        );
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
        db.set_source_file_inventory(ready_inventory(
            &db,
            &[
                "/workspace/project/settings.py",
                "/workspace/django/contrib/auth/__init__.py",
                "/workspace/blog/__init__.py",
            ],
        ));
        db.set_project_root_discovery(discovery(&db));
        let env = single_env_id(&db);

        let apps = installed_apps(&db, db.project(), env);

        assert_eq!(apps[0].entry(), "django.contrib.auth");
        assert_eq!(apps[1].entry(), "blog");
        assert_eq!(apps[0].module().as_str(), "django.contrib.auth");
        assert!(apps[0].config().is_none());
    }

    #[test]
    fn installed_apps_skip_unknown_segments() {
        let mut db = TestDb::with_project();
        db.set_file(
            "/workspace/project/settings.py",
            "INSTALLED_APPS = ['known', UNKNOWN]\n",
        );
        db.set_file("/workspace/known/__init__.py", "");
        db.set_source_file_inventory(ready_inventory(
            &db,
            &[
                "/workspace/project/settings.py",
                "/workspace/known/__init__.py",
            ],
        ));
        db.set_project_root_discovery(discovery(&db));
        let env = single_env_id(&db);

        let apps = installed_apps(&db, db.project(), env);

        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].entry(), "known");
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
        db.set_source_file_inventory(ready_inventory(
            &db,
            &["/workspace/project/settings.py", "/workspace/blog/apps.py"],
        ));
        db.set_project_root_discovery(discovery(&db));
        let env = single_env_id(&db);

        let apps = installed_apps(&db, db.project(), env);

        let config = apps[0].config().expect("app config should resolve");
        assert_eq!(apps[0].module().as_str(), "blog");
        assert_eq!(config.name(), Some("blog"));
        assert_eq!(config.label.as_deref(), Some("weblog"));
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
        db.set_source_file_inventory(ready_inventory(&db, &["/workspace/project/settings.py"]));
        let root = RootDiscoveryInput::new(
            &db,
            Utf8PathBuf::from("/workspace"),
            None,
            Some("project.settings".to_string()),
            Vec::new(),
            vec![Utf8PathBuf::from("/external")],
            ProjectEnvVars::default(),
            Vec::new(),
        );
        db.set_project_root_discovery(ProjectRootDiscovery::Ready(
            ProjectRootDiscoverySet::new(vec![root]).expect("root should create discovery"),
        ));
        let env = single_env_id(&db);

        let apps = installed_apps(&db, db.project(), env);

        assert!(apps.is_empty());
    }
}

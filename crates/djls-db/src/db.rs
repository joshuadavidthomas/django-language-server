//! Concrete Salsa database implementation for the Django Language Server.
//!
//! This module provides the concrete [`DjangoDatabase`] that implements all
//! the database traits from source, semantic, and project crates. This follows
//! Ruff's architecture pattern where the concrete database lives at the top level.

use std::sync::Arc;
use std::sync::Mutex;

use camino::Utf8Path;
use djls_conf::Settings;
use djls_project::Db as ProjectDb;
use djls_project::Project;
use djls_project::TemplateLibraries;
use djls_project::compute_model_graph;
use djls_project::template_libraries;
use djls_semantic::Db as SemanticDb;
use djls_semantic::TagSpecs;
use djls_semantic::compute_filter_arity_specs;
use djls_semantic::compute_tag_specs;
use djls_source::Db as SourceDb;
use djls_source::FileSystem;
use djls_source::SourceFiles;

/// Concrete Salsa database for the Django Language Server.
///
/// This database implements all the traits from various crates:
/// - [`SourceDb`] for file tracking and file reads
/// - [`SemanticDb`] for template semantics and diagnostics
/// - [`ProjectDb`] for project metadata and Python environment
#[salsa::db]
#[derive(Clone)]
pub struct DjangoDatabase {
    /// File system for reading file content (checks buffers first, then disk).
    fs: Arc<dyn FileSystem>,

    /// Registry of tracked files used by the workspace layer.
    files: SourceFiles,

    /// The single project for this database instance.
    ///
    /// This handle must remain stable for the lifetime of the database:
    /// tracked queries branch on the untracked `db.project()` read, so
    /// replacing the handle (or flipping None→Some after queries have run)
    /// changes results outside Salsa's dependency graph. Set once during
    /// construction; reloads mutate fields via Salsa setters through
    /// `Project::reload_from_settings`. Same invariant as ty's
    /// `ProjectDatabase` (`ty_project/src/db.rs`).
    project: Option<Project>,

    /// Configuration settings for the language server
    pub(crate) settings: Arc<Mutex<Settings>>,

    storage: salsa::Storage<Self>,

    // The logs are only used for testing and demonstrating reuse:
    #[cfg(test)]
    #[allow(dead_code)]
    logs: Arc<Mutex<Option<Vec<String>>>>,
}

#[cfg(test)]
impl Default for DjangoDatabase {
    fn default() -> Self {
        use djls_source::InMemoryFileSystem;

        let logs = <Arc<Mutex<Option<Vec<String>>>>>::default();

        Self {
            fs: Arc::new(InMemoryFileSystem::new()),
            files: SourceFiles::default(),
            project: None,
            settings: Arc::new(Mutex::new(Settings::default())),
            storage: salsa::Storage::new(Some(Box::new({
                let logs = logs.clone();
                move |event| {
                    eprintln!("Event: {event:?}");
                    // Log interesting events, if logging is enabled
                    if let Some(logs) = &mut *logs.lock().unwrap() {
                        // only log interesting events
                        if let salsa::EventKind::WillExecute { .. } = event.kind {
                            logs.push(format!("Event: {event:?}"));
                        }
                    }
                }
            }))),
            logs,
        }
    }
}

impl DjangoDatabase {
    /// Create a new [`DjangoDatabase`] with the given file system handle.
    ///
    /// The constructor creates protocol state only. When a project path is
    /// present, it installs a stable initial `Project` handle with root-only
    /// search paths and settings values that are already in memory. Applying
    /// project settings reloads disk-derived project fields onto the same
    /// stable handle.
    pub fn new(
        file_system: Arc<dyn FileSystem>,
        settings: &Settings,
        project_path: Option<&Utf8Path>,
    ) -> Self {
        let mut db = Self {
            fs: file_system,
            files: SourceFiles::default(),
            project: None,
            settings: Arc::new(Mutex::new(settings.clone())),
            storage: salsa::Storage::new(None),
            #[cfg(test)]
            logs: Arc::new(Mutex::new(None)),
        };

        if let Some(path) = project_path {
            db.set_project(path, settings);
        }

        db
    }

    fn set_project(&mut self, root: &Utf8Path, settings: &Settings) {
        let project = Project::initial(self, root, settings);
        self.project = Some(project);
    }
}

#[salsa::db]
impl salsa::Database for DjangoDatabase {}

#[salsa::db]
impl SourceDb for DjangoDatabase {
    fn files(&self) -> &SourceFiles {
        &self.files
    }

    fn file_system(&self) -> &dyn FileSystem {
        self.fs.as_ref()
    }
}

#[salsa::db]
impl SemanticDb for DjangoDatabase {
    fn tag_specs(&self) -> &TagSpecs {
        if let Some(project) = self.project() {
            compute_tag_specs(self, project)
        } else {
            static DEFAULT: std::sync::LazyLock<TagSpecs> =
                std::sync::LazyLock::new(djls_semantic::builtin_tag_specs);
            &DEFAULT
        }
    }

    fn diagnostics_config(&self) -> djls_conf::DiagnosticsConfig {
        self.settings().diagnostics().clone()
    }

    fn template_libraries(&self) -> &TemplateLibraries {
        self.project()
            .map_or(TemplateLibraries::empty_ref(), |project| {
                template_libraries(self, project)
            })
    }

    fn filter_arity_specs(&self) -> &djls_semantic::FilterAritySpecs {
        self.project()
            .map_or(djls_semantic::FilterAritySpecs::empty_ref(), |project| {
                compute_filter_arity_specs(self, project)
            })
    }

    fn model_graph(&self) -> &djls_project::ModelGraph {
        self.project()
            .map_or(djls_project::ModelGraph::empty_ref(), |project| {
                compute_model_graph(self, project)
            })
    }
}

#[salsa::db]
impl ProjectDb for DjangoDatabase {
    fn project(&self) -> Option<Project> {
        self.project
    }
}

#[cfg(test)]
mod marker_tests {
    // DjangoDatabase is intentionally !Sync — salsa::Storage uses RefCell
    // internally. Parallel work uses db.clone() per rayon task instead.

    #[test]
    fn db_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<crate::DjangoDatabase>();
    }
}

#[cfg(test)]
mod invalidation_tests {
    use std::collections::BTreeMap;
    use std::io;
    use std::sync::Arc;
    use std::sync::Mutex;

    use camino::Utf8Path;
    use camino::Utf8PathBuf;
    use djls_conf::Settings;
    use djls_project::Db as ProjectDb;
    use djls_project::Project;
    use djls_project::PythonModule;
    use djls_project::PythonModuleName;
    use djls_semantic::Db as SemanticDb;
    use djls_semantic::SemanticOffsetContext;
    use djls_semantic::template_inheritance;
    use djls_semantic::template_symbols;
    use djls_source::ChangeEvent;
    use djls_source::Db as SourceDb;
    use djls_source::File;
    use djls_source::FileSystem;
    use djls_source::InMemoryFileSystem;
    use djls_source::Offset;
    use djls_source::OsFileSystem;
    use djls_source::SourceChanges;
    use djls_source::SourceFiles;
    use djls_source::WalkOptions;
    use djls_source::path_to_file;
    use djls_templates::parse_template;
    use salsa::Database;
    use salsa::Setter;
    use tempfile::TempDir;
    use tempfile::tempdir;

    use super::DjangoDatabase;

    /// Captured Salsa events for test assertions.
    #[derive(Clone, Default)]
    struct EventLog {
        events: Arc<Mutex<Vec<salsa::Event>>>,
    }

    impl EventLog {
        fn take(&self) -> Vec<salsa::Event> {
            std::mem::take(&mut *self.events.lock().unwrap())
        }
    }

    struct CountingFileSystem {
        inner: InMemoryFileSystem,
        walk_counts: Mutex<BTreeMap<Utf8PathBuf, usize>>,
    }

    impl CountingFileSystem {
        fn new(inner: InMemoryFileSystem) -> Self {
            Self {
                inner,
                walk_counts: Mutex::new(BTreeMap::new()),
            }
        }

        fn walk_count(&self, root: &Utf8Path) -> usize {
            self.walk_counts
                .lock()
                .unwrap()
                .get(root)
                .copied()
                .unwrap_or_default()
        }
    }

    impl FileSystem for CountingFileSystem {
        fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
            self.inner.read_to_string(path)
        }

        fn exists(&self, path: &Utf8Path) -> bool {
            self.inner.exists(path)
        }

        fn is_file(&self, path: &Utf8Path) -> bool {
            self.inner.is_file(path)
        }

        fn is_dir(&self, path: &Utf8Path) -> bool {
            self.inner.is_dir(path)
        }

        fn case_sensitivity(&self) -> djls_source::CaseSensitivity {
            self.inner.case_sensitivity()
        }

        fn path_exists_case_sensitive(&self, path: &Utf8Path, prefix: &Utf8Path) -> bool {
            self.inner.path_exists_case_sensitive(path, prefix)
        }

        fn walk_root(&self, root: &Utf8Path, options: &WalkOptions) -> djls_source::RootWalk {
            *self
                .walk_counts
                .lock()
                .unwrap()
                .entry(root.to_path_buf())
                .or_default() += 1;
            self.inner.walk_root(root, options)
        }
    }

    /// Check whether a tracked query with the given name was executed
    /// (i.e., had a `WillExecute` event) in the captured events.
    fn was_executed(db: &DjangoDatabase, events: &[salsa::Event], query_name: &str) -> bool {
        events.iter().any(|event| match &event.kind {
            salsa::EventKind::WillExecute { database_key } => {
                let name = db.ingredient_debug_name(database_key.ingredient_index());
                name.contains(query_name)
            }
            _ => false,
        })
    }

    fn execution_count(db: &DjangoDatabase, events: &[salsa::Event], query_name: &str) -> usize {
        events
            .iter()
            .filter(|event| match &event.kind {
                salsa::EventKind::WillExecute { database_key } => {
                    let name = db.ingredient_debug_name(database_key.ingredient_index());
                    name.contains(query_name)
                }
                _ => false,
            })
            .count()
    }

    /// Create a test database with event logging and a pre-configured project.
    ///
    /// Uses `Interpreter::discover(None)` to match what `Project::bootstrap`
    /// produces, avoiding spurious interpreter mismatches from `$VIRTUAL_ENV`.
    fn test_db_with_project() -> (DjangoDatabase, EventLog) {
        let event_log = EventLog::default();
        let settings = Settings::default();

        let mut fs = InMemoryFileSystem::new();
        fs.add_file("/test/project/tags.py".into(), String::new());

        let mut db = DjangoDatabase {
            fs: Arc::new(fs),
            files: SourceFiles::default(),
            project: None,
            settings: Arc::new(Mutex::new(settings.clone())),
            storage: salsa::Storage::new(Some(Box::new({
                let log = event_log.clone();
                move |event| {
                    log.events.lock().unwrap().push(event);
                }
            }))),
            logs: Arc::new(Mutex::new(None)),
        };

        let project = Project::bootstrap(&db, "/test/project".into(), &settings);
        db.project = Some(project);

        (db, event_log)
    }

    struct TemplateInheritanceFixture {
        _tempdir: TempDir,
        db: DjangoDatabase,
        event_log: EventLog,
        fs: Arc<Mutex<InMemoryFileSystem>>,
        project: Project,
        child_file: File,
        parent_file: File,
        other_file: File,
        child_path: Utf8PathBuf,
        parent_path: Utf8PathBuf,
    }

    fn template_inheritance_fixture() -> TemplateInheritanceFixture {
        let event_log = EventLog::default();
        let tempdir = tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf()).unwrap();
        std::fs::write(
            root.join("djls.toml").as_std_path(),
            "django_settings_module = \"settings\"\n",
        )
        .unwrap();
        let settings = Settings::new(root.as_path(), None).unwrap();
        let templates_dir = root.join("templates");
        let child_path = templates_dir.join("child.html");
        let parent_path = templates_dir.join("base.html");
        let other_path = templates_dir.join("next.html");
        let fs = Arc::new(Mutex::new(InMemoryFileSystem::new()));
        {
            let mut fs = fs.lock().unwrap();
            fs.add_file(root.join("manage.py"), String::new());
            fs.add_file(
                root.join("settings.py"),
                format!(
                    "INSTALLED_APPS = []\nTEMPLATES = [{{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['{templates_dir}'], 'APP_DIRS': False}}]\n"
                ),
            );
            fs.add_file(
                child_path.clone(),
                "{% extends \"base.html\" %}\n{% block content %}{{ one }}{% endblock %}"
                    .to_string(),
            );
            fs.add_file(
                parent_path.clone(),
                "{% block content %}Base{% endblock %}".to_string(),
            );
            fs.add_file(
                other_path.clone(),
                "{% block content %}Next{% endblock %}".to_string(),
            );
        }

        let mut db = DjangoDatabase {
            fs: fs.clone(),
            files: SourceFiles::default(),
            project: None,
            settings: Arc::new(Mutex::new(settings.clone())),
            storage: salsa::Storage::new(Some(Box::new({
                let log = event_log.clone();
                move |event| {
                    log.events.lock().unwrap().push(event);
                }
            }))),
            logs: Arc::new(Mutex::new(None)),
        };
        let project = Project::bootstrap(&db, root.as_path(), &settings);
        db.project = Some(project);
        let child_file = path_to_file(&db, &child_path).expect("child fixture should exist");
        let parent_file = path_to_file(&db, &parent_path).expect("parent fixture should exist");
        let other_file = path_to_file(&db, &other_path).expect("other fixture should exist");

        TemplateInheritanceFixture {
            _tempdir: tempdir,
            db,
            event_log,
            fs,
            project,
            child_file,
            parent_file,
            other_file,
            child_path,
            parent_path,
        }
    }

    #[test]
    fn tag_specs_cached_on_repeated_access() {
        let (db, event_log) = test_db_with_project();

        // First call — should execute compute_tag_specs
        let _specs1 = db.tag_specs();
        let events = event_log.take();
        assert!(
            was_executed(&db, &events, "compute_tag_specs"),
            "compute_tag_specs should execute on first call"
        );

        // Second call — should be cached, no WillExecute
        let _specs2 = db.tag_specs();
        let events = event_log.take();
        assert!(
            !was_executed(&db, &events, "compute_tag_specs"),
            "compute_tag_specs should NOT re-execute on second call (cached)"
        );
    }

    #[test]
    fn settings_source_change_invalidates_template_library_extraction() {
        let event_log = EventLog::default();
        let tempdir = tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf()).unwrap();
        std::fs::write(
            root.join("djls.toml").as_std_path(),
            "django_settings_module = \"settings\"\n",
        )
        .unwrap();
        let settings = Settings::new(root.as_path(), None).unwrap();
        let settings_path = root.join("settings.py");
        let builtin_path = root.join("project_tags.py");

        let fs = Arc::new(Mutex::new(InMemoryFileSystem::new()));
        {
            let mut fs = fs.lock().unwrap();
            fs.add_file(root.join("manage.py"), String::new());
            fs.add_file(
                settings_path.clone(),
                "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['project_tags']}}]\n".to_string(),
            );
            fs.add_file(
                builtin_path,
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef project_tag():\n    pass\n".to_string(),
            );
        }

        let mut db = DjangoDatabase {
            fs: fs.clone(),
            files: SourceFiles::default(),
            project: None,
            settings: Arc::new(Mutex::new(settings.clone())),
            storage: salsa::Storage::new(Some(Box::new({
                let log = event_log.clone();
                move |event| {
                    log.events.lock().unwrap().push(event);
                }
            }))),
            logs: Arc::new(Mutex::new(None)),
        };
        let project = Project::bootstrap(&db, root.as_path(), &settings);
        db.project = Some(project);

        let specs = db.tag_specs();
        assert!(
            specs.get("project_tag").is_some(),
            "configured builtin tag should be extracted before settings change"
        );
        event_log.take();

        let settings_file = path_to_file(&db, &settings_path).expect("fixture file should exist");
        fs.lock().unwrap().add_file(
            settings_path,
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'builtins': []}}]\n".to_string(),
        );
        SourceChanges::new([ChangeEvent::ContentChanged(settings_file.path(&db).clone())])
            .apply(&mut db);

        let specs = db.tag_specs();
        assert!(
            specs.get("project_tag").is_none(),
            "removed configured builtin tag should no longer be extracted"
        );
        let events = event_log.take();
        assert!(
            was_executed(&db, &events, "template_libraries"),
            "template_libraries should re-execute after the settings source changes"
        );
        assert!(
            was_executed(&db, &events, "compute_tag_specs"),
            "tag specs should re-execute after Template Library facts change"
        );
    }

    #[test]
    fn root_revision_change_invalidates_project_template_files() {
        let source = "{% extends \"base.html\" %}";
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(
            "/test/project/templates/child.html".into(),
            source.to_string(),
        );
        fs.add_file(
            "/test/project/templates/base.html".into(),
            "base".to_string(),
        );

        let event_log = EventLog::default();
        let settings = Settings::default();
        let mut db = DjangoDatabase {
            fs: Arc::new(fs),
            files: SourceFiles::default(),
            project: None,
            settings: Arc::new(Mutex::new(settings.clone())),
            storage: salsa::Storage::new(Some(Box::new({
                let log = event_log.clone();
                move |event| {
                    log.events.lock().unwrap().push(event);
                }
            }))),
            logs: Arc::new(Mutex::new(None)),
        };
        let project = Project::bootstrap(&db, "/test/project".into(), &settings);
        db.project = Some(project);

        let project = db.project.unwrap();

        let child_path = Utf8Path::new("/test/project/templates/child.html");
        let child = path_to_file(&db, child_path).expect("fixture file should exist");
        let offset = Offset::try_from(source.find("base.html").unwrap()).unwrap();
        {
            let SemanticOffsetContext::TemplateReference { name, .. } =
                SemanticOffsetContext::from_offset(&db, child, offset)
            else {
                panic!("expected extends argument to be a template reference");
            };
            let _result = djls_project::template_resolution(&db, project).resolve(&db, name);
        }
        event_log.take();

        let root = db.files().expect_root(&db, child_path);
        db.bump_file_root_revision(root);

        let SemanticOffsetContext::TemplateReference { name, .. } =
            SemanticOffsetContext::from_offset(&db, child, offset)
        else {
            panic!("expected extends argument to be a template reference");
        };
        let _result = djls_project::template_resolution(&db, project).resolve(&db, name);
        let events = event_log.take();
        assert!(
            was_executed(&db, &events, "project_template_files"),
            "project_template_files should re-execute after the search root revision changes"
        );
    }

    #[test]
    fn template_resolution_views_share_one_walk_per_root_revision() {
        let tempdir = tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf()).unwrap();
        std::fs::write(
            root.join("djls.toml").as_std_path(),
            "django_settings_module = \"settings\"\n",
        )
        .unwrap();
        let settings = Settings::new(root.as_path(), None).unwrap();
        let templates_dir = root.join("templates");

        let mut inner = InMemoryFileSystem::new();
        inner.add_file(root.join("manage.py"), String::new());
        inner.add_file(
            root.join("settings.py"),
            format!(
                "INSTALLED_APPS = []\nTEMPLATES = [{{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['{templates_dir}'], 'APP_DIRS': False}}]\n"
            ),
        );
        inner.add_file(templates_dir.join("base.html"), "base".to_string());
        let fs = Arc::new(CountingFileSystem::new(inner));

        let mut db = DjangoDatabase {
            fs: fs.clone(),
            files: SourceFiles::default(),
            project: None,
            settings: Arc::new(Mutex::new(settings.clone())),
            storage: salsa::Storage::default(),
            logs: Arc::new(Mutex::new(None)),
        };
        let project = Project::bootstrap(&db, root.as_path(), &settings);
        db.project = Some(project);
        {
            let resolution = djls_project::template_resolution(&db, project);
            let name = djls_project::TemplateName::new(&db, "base.html".to_string());
            assert_eq!(resolution.origins(&db).count(), 1);
            assert_eq!(resolution.template_names(&db).count(), 1);
            assert_eq!(resolution.origins_for_name(&db, name).len(), 1);
            assert!(matches!(
                resolution.resolve(&db, name),
                djls_project::FindTemplateResult::Found(_)
            ));
        }
        assert_eq!(
            fs.walk_count(&templates_dir),
            1,
            "all resolution views should reuse the same directory index"
        );

        let source_root = db.files().expect_root(&db, templates_dir.as_path());
        db.bump_file_root_revision(source_root);

        {
            let resolution = djls_project::template_resolution(&db, project);
            let name = djls_project::TemplateName::new(&db, "base.html".to_string());
            assert_eq!(resolution.origins(&db).count(), 1);
            assert_eq!(resolution.template_names(&db).count(), 1);
            assert_eq!(resolution.origins_for_name(&db, name).len(), 1);
            assert!(matches!(
                resolution.resolve(&db, name),
                djls_project::FindTemplateResult::Found(_)
            ));
        }
        assert_eq!(
            fs.walk_count(&templates_dir),
            2,
            "a relevant root revision bump should trigger exactly one additional walk"
        );
    }

    #[test]
    fn template_directories_reports_incomplete_derivation() {
        let tempdir = tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf()).unwrap();
        std::fs::write(
            root.join("djls.toml").as_std_path(),
            "django_settings_module = \"settings\"\n",
        )
        .unwrap();
        let settings = Settings::new(root.as_path(), None).unwrap();
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(root.join("manage.py"), String::new());

        let mut db = DjangoDatabase {
            fs: Arc::new(fs),
            files: SourceFiles::default(),
            project: None,
            settings: Arc::new(Mutex::new(settings.clone())),
            storage: salsa::Storage::default(),
            logs: Arc::new(Mutex::new(None)),
        };
        let project = Project::bootstrap(&db, root.as_path(), &settings);
        db.project = Some(project);

        assert!(djls_project::template_directories(&db, project).configuration_may_omit_roots());
    }

    #[test]
    fn template_directories_does_not_index_templates() {
        let event_log = EventLog::default();
        let tempdir = tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf()).unwrap();
        std::fs::write(
            root.join("djls.toml").as_std_path(),
            "django_settings_module = \"settings\"\n",
        )
        .unwrap();
        let settings = Settings::new(root.as_path(), None).unwrap();
        let settings_path = root.join("settings.py");
        let templates_dir = root.join("templates");

        let mut fs = InMemoryFileSystem::new();
        fs.add_file(root.join("manage.py"), String::new());
        fs.add_file(
            settings_path,
            format!(
                "INSTALLED_APPS = []\nTEMPLATES = [{{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['{templates_dir}'], 'APP_DIRS': False}}]\n"
            ),
        );
        fs.add_file(templates_dir.join("base.html"), "base".to_string());

        let mut db = DjangoDatabase {
            fs: Arc::new(fs),
            files: SourceFiles::default(),
            project: None,
            settings: Arc::new(Mutex::new(settings.clone())),
            storage: salsa::Storage::new(Some(Box::new({
                let log = event_log.clone();
                move |event| {
                    log.events.lock().unwrap().push(event);
                }
            }))),
            logs: Arc::new(Mutex::new(None)),
        };
        let project = Project::bootstrap(&db, root.as_path(), &settings);
        db.project = Some(project);
        event_log.take();

        assert_eq!(
            djls_project::template_directories(&db, project)
                .known_roots()
                .collect::<Vec<_>>(),
            [templates_dir.as_path()]
        );
        let events = event_log.take();
        assert!(
            was_executed(&db, &events, "template_directories"),
            "template_directories should derive the trusted directory list"
        );
        assert!(
            !was_executed(&db, &events, "project_template_files"),
            "template_directories should not walk template files"
        );
    }

    #[test]
    fn settings_source_change_invalidates_template_directories() {
        let event_log = EventLog::default();
        let tempdir = tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf()).unwrap();
        std::fs::write(
            root.join("djls.toml").as_std_path(),
            "django_settings_module = \"settings\"\n",
        )
        .unwrap();
        let settings = Settings::new(root.as_path(), None).unwrap();
        let settings_path = root.join("settings.py");
        let templates_dir = root.join("templates");
        let other_templates_dir = root.join("other_templates");

        let fs = Arc::new(Mutex::new(InMemoryFileSystem::new()));
        {
            let mut fs = fs.lock().unwrap();
            fs.add_file(root.join("manage.py"), String::new());
            fs.add_file(
                settings_path.clone(),
                format!(
                    "INSTALLED_APPS = []\nTEMPLATES = [{{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['{templates_dir}'], 'APP_DIRS': False}}]\n"
                ),
            );
            fs.add_file(templates_dir.join("base.html"), "base".to_string());
            fs.add_file(root.join("other.py"), "VALUE = 1\n".to_string());
        }

        let mut db = DjangoDatabase {
            fs: fs.clone(),
            files: SourceFiles::default(),
            project: None,
            settings: Arc::new(Mutex::new(settings.clone())),
            storage: salsa::Storage::new(Some(Box::new({
                let log = event_log.clone();
                move |event| {
                    log.events.lock().unwrap().push(event);
                }
            }))),
            logs: Arc::new(Mutex::new(None)),
        };
        let project = Project::bootstrap(&db, root.as_path(), &settings);
        db.project = Some(project);

        assert_eq!(
            djls_project::template_directories(&db, project)
                .known_roots()
                .collect::<Vec<_>>(),
            [templates_dir.as_path()]
        );
        event_log.take();

        let settings_file = path_to_file(&db, &settings_path).expect("fixture file should exist");
        fs.lock().unwrap().add_file(
            settings_path,
            format!(
                "INSTALLED_APPS = []\nTEMPLATES = [{{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['{other_templates_dir}'], 'APP_DIRS': False}}]\n"
            ),
        );
        SourceChanges::new([ChangeEvent::ContentChanged(settings_file.path(&db).clone())])
            .apply(&mut db);

        assert_eq!(
            djls_project::template_directories(&db, project)
                .known_roots()
                .collect::<Vec<_>>(),
            [other_templates_dir.as_path()]
        );
        let events = event_log.take();
        assert!(
            was_executed(&db, &events, "template_directories"),
            "template_directories should re-execute after the settings source changes"
        );
    }

    #[test]
    fn unrelated_file_revision_does_not_invalidate_template_directories() {
        let event_log = EventLog::default();
        let tempdir = tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf()).unwrap();
        std::fs::write(
            root.join("djls.toml").as_std_path(),
            "django_settings_module = \"settings\"\n",
        )
        .unwrap();
        let settings = Settings::new(root.as_path(), None).unwrap();
        let settings_path = root.join("settings.py");
        let templates_dir = root.join("templates");
        let other_path = root.join("other.py");

        let mut fs = InMemoryFileSystem::new();
        fs.add_file(root.join("manage.py"), String::new());
        fs.add_file(
            settings_path,
            format!(
                "INSTALLED_APPS = []\nTEMPLATES = [{{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['{templates_dir}'], 'APP_DIRS': False}}]\n"
            ),
        );
        fs.add_file(templates_dir.join("base.html"), "base".to_string());
        fs.add_file(other_path.clone(), "VALUE = 1\n".to_string());

        let mut db = DjangoDatabase {
            fs: Arc::new(fs),
            files: SourceFiles::default(),
            project: None,
            settings: Arc::new(Mutex::new(settings.clone())),
            storage: salsa::Storage::new(Some(Box::new({
                let log = event_log.clone();
                move |event| {
                    log.events.lock().unwrap().push(event);
                }
            }))),
            logs: Arc::new(Mutex::new(None)),
        };
        let project = Project::bootstrap(&db, root.as_path(), &settings);
        db.project = Some(project);

        assert_eq!(
            djls_project::template_directories(&db, project)
                .known_roots()
                .collect::<Vec<_>>(),
            [templates_dir.as_path()]
        );
        event_log.take();

        let other_file = path_to_file(&db, &other_path).expect("fixture file should exist");
        db.bump_file_revision(other_file);

        assert_eq!(
            djls_project::template_directories(&db, project)
                .known_roots()
                .collect::<Vec<_>>(),
            [root.join("templates")]
        );
        let events = event_log.take();
        assert!(
            !was_executed(&db, &events, "template_directories"),
            "template_directories should stay cached after an unrelated file revision changes"
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn module_detail_perturbations_do_not_recompute_settings_identity_consumers() {
        let event_log = EventLog::default();
        let tempdir = tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf()).unwrap();
        let vendor = root.join("vendor");
        std::fs::write(
            root.join("djls.toml").as_std_path(),
            format!("django_settings_module = \"unrelated\"\npythonpath = [\"{vendor}\"]\n"),
        )
        .unwrap();
        let settings = Settings::new(root.as_path(), None).unwrap();
        let unrelated_path = root.join("unrelated.py");

        let fs = Arc::new(Mutex::new(InMemoryFileSystem::new()));
        {
            let mut fs = fs.lock().unwrap();
            fs.add_file(root.join("manage.py"), String::new());
            fs.add_file(
                unrelated_path.clone(),
                "INSTALLED_APPS = []\nTEMPLATES = []\n".to_string(),
            );
            fs.add_file(vendor.join("anchor.py"), String::new());
        }

        let mut db = DjangoDatabase {
            fs: fs.clone(),
            files: SourceFiles::default(),
            project: None,
            settings: Arc::new(Mutex::new(settings.clone())),
            storage: salsa::Storage::new(Some(Box::new({
                let log = event_log.clone();
                move |event| {
                    log.events.lock().unwrap().push(event);
                }
            }))),
            logs: Arc::new(Mutex::new(None)),
        };
        let project = Project::bootstrap(&db, root.as_path(), &settings);
        db.project = Some(project);
        let name = PythonModuleName::parse("unrelated").unwrap();

        let module = PythonModule::resolve(&db, project, name.clone())
            .expect("unrelated should resolve from the project root");
        assert_eq!(module.path(), unrelated_path.as_path());
        assert_eq!(
            djls_project::testing::settings_module_file(&db, project)
                .expect("settings module should resolve")
                .path(&db),
            unrelated_path.as_path()
        );
        let _ = djls_project::testing::django_settings(&db, project);
        let vendor_root = db
            .files()
            .root(&db, vendor.as_path())
            .expect("vendor search path root should be registered");
        event_log.take();

        fs.lock()
            .unwrap()
            .add_file(vendor.join("unrelated/marker.txt"), String::new());
        db.bump_file_root_revision(vendor_root);

        let module = PythonModule::resolve(&db, project, name.clone())
            .expect("unrelated should stay resolved from the project root");
        assert_eq!(module.path(), unrelated_path.as_path());
        assert_eq!(
            djls_project::testing::settings_module_file(&db, project)
                .expect("settings module should stay resolved")
                .path(&db),
            unrelated_path.as_path()
        );
        let _ = djls_project::testing::django_settings(&db, project);
        let events = event_log.take();
        assert!(
            !was_executed(&db, &events, "settings_module_file"),
            "settings_module_file should backdate through unchanged module identity"
        );
        assert!(
            !was_executed(&db, &events, "django_settings"),
            "django_settings should not recompute for detail-only module candidate churn"
        );

        fs.lock()
            .unwrap()
            .add_file(vendor.join("unrelated.py"), String::new());
        db.bump_file_root_revision(vendor_root);

        let module = PythonModule::resolve(&db, project, name)
            .expect("unrelated should stay resolved from the project root");
        assert_eq!(module.path(), unrelated_path.as_path());
        assert_eq!(
            djls_project::testing::settings_module_file(&db, project)
                .expect("settings module should stay resolved")
                .path(&db),
            unrelated_path.as_path()
        );
        let _ = djls_project::testing::django_settings(&db, project);
        let events = event_log.take();
        assert!(
            !was_executed(&db, &events, "settings_module_file"),
            "settings_module_file should backdate through unchanged first-root identity"
        );
        assert!(
            !was_executed(&db, &events, "django_settings"),
            "django_settings should not recompute when the selected module identity is unchanged"
        );
    }

    #[test]
    fn tagspecs_settings_change_reports_semantic_change() {
        let tempdir = tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf()).unwrap();
        std::fs::write(
            root.join("djls.toml").as_std_path(),
            r#"
[tagspecs]
version = "0.6.0"

[[tagspecs.libraries]]
module = "myapp.templatetags.custom"

[[tagspecs.libraries.tags]]
name = "switch"
type = "block"
"#,
        )
        .unwrap();

        let mut db = DjangoDatabase::new(
            Arc::new(InMemoryFileSystem::new()),
            &Settings::default(),
            Some(root.as_path()),
        );
        let settings = Settings::new(root.as_path(), None).unwrap();

        db.apply_project_settings(settings);

        let project = db.project().expect("project should exist");
        assert!(
            project
                .tagspecs(&db)
                .libraries
                .iter()
                .any(|library| library.module == "myapp.templatetags.custom")
        );
    }

    #[test]
    fn initial_project_loads_disk_facts_into_same_handle() {
        let tempdir = tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf()).unwrap();
        let extra_path = root.join("extra_python");
        std::fs::create_dir_all(extra_path.as_std_path()).unwrap();
        std::fs::write(root.join(".env.local").as_std_path(), "FROM_ENV=loaded\n").unwrap();
        std::fs::write(
            root.join("djls.toml").as_std_path(),
            format!(
                r#"
django_settings_module = "config.settings"
pythonpath = ["{extra_path}"]
env_file = ".env.local"
"#
            ),
        )
        .unwrap();

        let mut db = DjangoDatabase::new(
            Arc::new(OsFileSystem::default()),
            &Settings::default(),
            Some(root.as_path()),
        );
        let project = db.project().expect("initial project should exist");

        assert_eq!(project.root(&db), root.as_path());
        assert_eq!(project.django_settings_module(&db).as_ref(), None);
        assert!(project.pythonpath(&db).is_empty());
        assert!(project.env_vars(&db).is_empty());
        let initial_paths: Vec<_> = project
            .search_paths(&db)
            .iter()
            .map(|search_path| search_path.path().to_path_buf())
            .collect();
        assert_eq!(initial_paths, vec![root.clone()]);

        let settings = Settings::new(root.as_path(), None).unwrap();
        db.apply_project_settings(settings);
        let environment = djls_project::testing::compute_django_environment(&db, project);
        djls_project::apply_django_environment(&mut db, environment);

        assert_eq!(db.project(), Some(project));
        assert_eq!(
            project
                .django_settings_module(&db)
                .as_ref()
                .map(djls_project::PythonModuleName::as_str),
            Some("config.settings")
        );
        assert_eq!(project.pythonpath(&db), &vec![extra_path.clone()]);
        assert_eq!(
            project.env_vars(&db),
            &vec![("FROM_ENV".to_string(), "loaded".to_string())]
        );
        let loaded_paths: Vec<_> = project
            .search_paths(&db)
            .iter()
            .map(|search_path| search_path.path().to_path_buf())
            .collect();
        assert_eq!(loaded_paths.first(), Some(&root));
        assert!(loaded_paths.contains(&extra_path));
    }

    #[test]
    fn tagspecs_change_invalidates_compute_tag_specs() {
        let (mut db, event_log) = test_db_with_project();

        // Prime the cache
        let _specs = db.tag_specs();
        event_log.take();

        let project = db.project.unwrap();

        let new_tagspecs = djls_conf::TagSpecDef {
            version: "0.6.0".to_string(),
            engine: "django".to_string(),
            requires_engine: None,
            extends: vec![],
            libraries: vec![djls_conf::TagLibraryDef {
                module: "myapp.templatetags.custom".to_string(),
                requires_engine: None,
                tags: vec![djls_conf::TagDef {
                    name: "switch".to_string(),
                    tag_type: djls_conf::TagTypeDef::Block,
                    end: None,
                    intermediates: vec![],
                    args: vec![],
                    extra: None,
                }],
                extra: None,
            }],
            extra: None,
        };

        project.set_tagspecs(&mut db).to(new_tagspecs);

        // Access again — should re-execute
        let _specs = db.tag_specs();
        let events = event_log.take();
        assert!(
            was_executed(&db, &events, "compute_tag_specs"),
            "compute_tag_specs should re-execute after tagspecs change"
        );
    }

    #[test]
    fn same_value_no_invalidation() {
        let (db, event_log) = test_db_with_project();

        // Prime the cache
        let _specs = db.tag_specs();
        event_log.take();

        // Simulate a no-op update path: compare against an identical value and
        // intentionally skip any setter call.
        let project = db.project.unwrap();
        let current = project.tagspecs(&db).clone();

        assert_eq!(project.tagspecs(&db), &current);
        // No setter called — cache should be preserved

        let _specs = db.tag_specs();
        let events = event_log.take();
        assert!(
            !was_executed(&db, &events, "compute_tag_specs"),
            "compute_tag_specs should NOT re-execute when value is unchanged"
        );
    }

    #[test]
    fn project_settings_unchanged_no_invalidation() {
        let (mut db, event_log) = test_db_with_project();

        // Prime the cache
        let _specs = db.tag_specs();
        event_log.take();

        // Apply default project settings, matching what the project was created with.
        let settings = Settings::default();
        db.project()
            .expect("project should exist")
            .reload_from_settings(&mut db, &settings);

        // Access tag_specs — should still be cached
        let _specs = db.tag_specs();
        let events = event_log.take();
        assert!(
            !was_executed(&db, &events, "compute_tag_specs"),
            "compute_tag_specs should NOT re-execute when settings are unchanged"
        );
    }

    #[test]
    fn filter_arities_cached_on_repeated_access() {
        let (db, event_log) = test_db_with_project();

        // Create a Python file and track it
        let file = path_to_file(&db, camino::Utf8Path::new("/test/project/tags.py"))
            .expect("fixture file should exist");

        // First extraction
        let _result1 = djls_project::extract_filter_arities(
            &db,
            file,
            djls_project::PythonModuleName::parse("test.project.tags").unwrap(),
        );
        let events = event_log.take();
        assert!(
            was_executed(&db, &events, "extract_filter_arities"),
            "extract_filter_arities should execute on first call"
        );

        // Second call — cached
        let _result2 = djls_project::extract_filter_arities(
            &db,
            file,
            djls_project::PythonModuleName::parse("test.project.tags").unwrap(),
        );
        let events = event_log.take();
        assert!(
            !was_executed(&db, &events, "extract_filter_arities"),
            "extract_filter_arities should NOT re-execute on second call (cached)"
        );
    }

    #[test]
    fn file_revision_change_with_same_source_backdates() {
        let (mut db, event_log) = test_db_with_project();

        // Create and extract from a file (file doesn't exist, source is empty)
        let file = path_to_file(&db, camino::Utf8Path::new("/test/project/tags.py"))
            .expect("fixture file should exist");
        let _result = djls_project::extract_filter_arities(
            &db,
            file,
            djls_project::PythonModuleName::parse("test.project.tags").unwrap(),
        );
        event_log.take();

        // Bump the file revision — but the source is still empty (file not in FS)
        file.set_revision(&mut db).to(1);

        // Salsa's backdate optimization: file.try_source() returns the same empty text,
        // so extract_filter_arities does NOT re-execute (correct behavior)
        let _result = djls_project::extract_filter_arities(
            &db,
            file,
            djls_project::PythonModuleName::parse("test.project.tags").unwrap(),
        );
        let events = event_log.take();
        assert!(
            !was_executed(&db, &events, "extract_filter_arities"),
            "extract_filter_arities should NOT re-execute when source content is unchanged (backdate)"
        );
    }

    #[test]
    fn file_with_different_content_produces_different_extraction() {
        use djls_source::InMemoryFileSystem;

        // Create FS with a Python file
        let mut fs = InMemoryFileSystem::new();
        fs.add_file(
            "/test/project/tags.py".into(),
            r"
from django import template
register = template.Library()

@register.filter
def my_filter(value, arg):
    return value + arg
"
            .to_string(),
        );

        let event_log = EventLog::default();
        let settings = Settings::default();

        let db = DjangoDatabase {
            fs: Arc::new(fs),
            files: SourceFiles::default(),
            project: None,
            settings: Arc::new(Mutex::new(settings.clone())),
            storage: salsa::Storage::new(Some(Box::new({
                let log = event_log.clone();
                move |event| {
                    log.events.lock().unwrap().push(event);
                }
            }))),
            logs: Arc::new(Mutex::new(None)),
        };

        let file = path_to_file(&db, camino::Utf8Path::new("/test/project/tags.py"))
            .expect("fixture file should exist");
        let result = djls_project::extract_filter_arities(
            &db,
            file,
            djls_project::PythonModuleName::parse("test.project.tags").unwrap(),
        );

        // Should extract the filter
        let key = djls_project::SymbolKey::filter("test.project.tags", "my_filter");
        assert!(
            result.arities().contains_key(&key),
            "should extract filter from file content"
        );
        assert!(result.arities()[&key].expects_arg);

        let other_module_result = djls_project::extract_filter_arities(
            &db,
            file,
            djls_project::PythonModuleName::parse("other.project.tags").unwrap(),
        );
        let other_key = djls_project::SymbolKey::filter("other.project.tags", "my_filter");
        assert!(other_module_result.arities().contains_key(&other_key));
        assert!(!other_module_result.arities().contains_key(&key));
    }

    fn settings_with_custom_library(module_name: &str) -> String {
        "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'custom': '__MODULE__'}}}]\n"
            .replace("__MODULE__", module_name)
    }

    fn template_tag_library_source(tag_name: &str) -> String {
        format!(
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef {tag_name}():\n    pass\n"
        )
    }

    fn assert_custom_library_module(db: &DjangoDatabase, module_name: &str) {
        match db.template_libraries().loadable_library_str("custom") {
            djls_project::LoadableLibraryLookup::Found(custom) => {
                assert_eq!(custom.module_name_str(), module_name);
            }
            djls_project::LoadableLibraryLookup::Inconclusive(candidates) => {
                assert_eq!(candidates.len(), 1);
                assert_eq!(candidates[0].module_name_str(), module_name);
            }
            djls_project::LoadableLibraryLookup::Ambiguous(candidates) => {
                panic!("custom library should not be ambiguous: {candidates:?}");
            }
            djls_project::LoadableLibraryLookup::Absent => {
                panic!("custom library candidate should be known");
            }
        }
    }

    fn assert_django_discovery_updates_star_imported_settings_source(settings_source: &str) {
        let tempdir = tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf()).unwrap();
        std::fs::write(
            root.join("djls.toml").as_std_path(),
            "django_settings_module = \"settings\"\n",
        )
        .unwrap();
        let settings = Settings::new(root.as_path(), None).unwrap();
        let settings_path = root.join("settings.py");
        let base_settings_path = root.join("base.py");

        let fs = Arc::new(Mutex::new(InMemoryFileSystem::new()));
        {
            let mut fs = fs.lock().unwrap();
            fs.add_file(root.join("manage.py"), String::new());
            fs.add_file(settings_path, settings_source.to_string());
            fs.add_file(
                base_settings_path.clone(),
                settings_with_custom_library("old_tags"),
            );
            fs.add_file(
                root.join("old_tags.py"),
                template_tag_library_source("old_tag"),
            );
            fs.add_file(
                root.join("new_tags.py"),
                template_tag_library_source("new_tag"),
            );
        }

        let mut db = DjangoDatabase {
            fs: fs.clone(),
            files: SourceFiles::default(),
            project: None,
            settings: Arc::new(Mutex::new(settings.clone())),
            storage: salsa::Storage::default(),
            logs: Arc::new(Mutex::new(None)),
        };
        let project = Project::bootstrap(&db, root.as_path(), &settings);
        db.project = Some(project);

        assert_custom_library_module(&db, "old_tags");

        fs.lock()
            .unwrap()
            .add_file(base_settings_path, settings_with_custom_library("new_tags"));
        apply_project_discovery(&mut db);

        assert_custom_library_module(&db, "new_tags");
    }

    #[test]
    fn django_discovery_reads_changed_star_imported_settings_source_for_template_libraries() {
        assert_django_discovery_updates_star_imported_settings_source("from .base import *\n");
    }

    #[test]
    fn django_discovery_reads_changed_try_star_imported_settings_source() {
        assert_django_discovery_updates_star_imported_settings_source(
            "try:\n    from .base import *\nexcept ImportError:\n    pass\n",
        );
    }

    #[test]
    fn django_discovery_reads_changed_conditionally_star_imported_settings_source() {
        assert_django_discovery_updates_star_imported_settings_source(
            "import os\nif os.environ.get(\"EXTRA\"):\n    from .base import *\nelse:\n    from .base import *\n",
        );
    }

    #[test]
    fn django_discovery_discovers_newly_star_imported_known_file() {
        let tempdir = tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf()).unwrap();
        std::fs::write(
            root.join("djls.toml").as_std_path(),
            "django_settings_module = \"settings\"\n",
        )
        .unwrap();
        let settings = Settings::new(root.as_path(), None).unwrap();
        let settings_path = root.join("settings.py");
        let extra_settings_path = root.join("extra.py");

        let fs = Arc::new(Mutex::new(InMemoryFileSystem::new()));
        {
            let mut fs = fs.lock().unwrap();
            fs.add_file(root.join("manage.py"), String::new());
            fs.add_file(settings_path.clone(), "INSTALLED_APPS = []\n".to_string());
            fs.add_file(
                extra_settings_path.clone(),
                settings_with_custom_library("old_tags"),
            );
            fs.add_file(
                root.join("old_tags.py"),
                template_tag_library_source("old_tag"),
            );
            fs.add_file(
                root.join("new_tags.py"),
                template_tag_library_source("new_tag"),
            );
        }

        let mut db = DjangoDatabase {
            fs: fs.clone(),
            files: SourceFiles::default(),
            project: None,
            settings: Arc::new(Mutex::new(settings.clone())),
            storage: salsa::Storage::default(),
            logs: Arc::new(Mutex::new(None)),
        };
        let project = Project::bootstrap(&db, root.as_path(), &settings);
        db.project = Some(project);

        assert!(
            db.template_libraries()
                .loadable_library_str("custom")
                .found()
                .is_none()
        );
        let extra_file =
            path_to_file(&db, &extra_settings_path).expect("fixture file should exist");
        assert!(
            extra_file
                .try_source(&db)
                .expect("extra template tag file should be readable")
                .as_str()
                .contains("old_tags")
        );

        {
            let mut fs = fs.lock().unwrap();
            fs.add_file(settings_path, "from .extra import *\n".to_string());
            fs.add_file(
                extra_settings_path,
                settings_with_custom_library("new_tags"),
            );
        }
        apply_project_discovery(&mut db);

        assert_custom_library_module(&db, "new_tags");
    }

    #[test]
    fn one_reload_observes_deleted_and_recreated_settings_import() {
        let tempdir = tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf()).unwrap();
        std::fs::write(
            root.join("djls.toml").as_std_path(),
            "django_settings_module = \"settings\"\n",
        )
        .unwrap();
        let settings = Settings::new(root.as_path(), None).unwrap();
        let base_settings_path = root.join("base.py");

        let fs = Arc::new(Mutex::new(InMemoryFileSystem::new()));
        {
            let mut fs = fs.lock().unwrap();
            fs.add_file(root.join("manage.py"), String::new());
            fs.add_file(
                root.join("settings.py"),
                "from .base import *\n".to_string(),
            );
            fs.add_file(
                base_settings_path.clone(),
                settings_with_custom_library("old_tags"),
            );
            fs.add_file(
                root.join("old_tags.py"),
                template_tag_library_source("old_tag"),
            );
            fs.add_file(
                root.join("new_tags.py"),
                template_tag_library_source("new_tag"),
            );
        }

        let mut db = DjangoDatabase {
            fs: fs.clone(),
            files: SourceFiles::default(),
            project: None,
            settings: Arc::new(Mutex::new(settings.clone())),
            storage: salsa::Storage::default(),
            logs: Arc::new(Mutex::new(None)),
        };
        let project = Project::bootstrap(&db, root.as_path(), &settings);
        db.project = Some(project);
        assert_custom_library_module(&db, "old_tags");

        fs.lock().unwrap().remove_file(&base_settings_path);
        apply_project_discovery(&mut db);
        assert!(
            db.template_libraries()
                .loadable_library_str("custom")
                .found()
                .is_none()
        );

        fs.lock()
            .unwrap()
            .add_file(base_settings_path, settings_with_custom_library("new_tags"));
        apply_project_discovery(&mut db);
        assert_custom_library_module(&db, "new_tags");
    }

    fn apply_project_discovery(db: &mut DjangoDatabase) {
        let project = db.project().expect("project should exist");
        let environment = djls_project::testing::compute_django_environment(db, project);
        djls_project::apply_django_environment(db, environment);
        let _facts = djls_project::testing::compute_project_facts(db, project);
    }

    #[test]
    fn environment_apply_bumps_roots_but_not_unchanged_file_contents() {
        let tempdir = tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf()).unwrap();
        std::fs::write(
            root.join("djls.toml").as_std_path(),
            "django_settings_module = \"settings\"\n",
        )
        .unwrap();
        let settings = Settings::new(root.as_path(), None).unwrap();
        let tag_path = root.join("blog/templatetags/custom.py");

        let fs = Arc::new(Mutex::new(InMemoryFileSystem::new()));
        {
            let mut fs = fs.lock().unwrap();
            fs.add_file(root.join("manage.py"), String::new());
            fs.add_file(
                root.join("settings.py"),
                "INSTALLED_APPS = ['blog']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n".to_string(),
            );
            fs.add_file(root.join("blog/__init__.py"), String::new());
            fs.add_file(root.join("blog/templatetags/__init__.py"), String::new());
            fs.add_file(tag_path, template_tag_library_source("custom_tag"));
        }

        let mut db = DjangoDatabase {
            fs,
            files: SourceFiles::default(),
            project: None,
            settings: Arc::new(Mutex::new(settings.clone())),
            storage: salsa::Storage::default(),
            logs: Arc::new(Mutex::new(None)),
        };
        let project = Project::bootstrap(&db, root.as_path(), &settings);
        db.project = Some(project);

        let environment = djls_project::testing::compute_django_environment(&db, project);
        djls_project::apply_django_environment(&mut db, environment);
        let facts = djls_project::testing::compute_project_facts(&db, project);
        let file_paths: Vec<_> = facts.file_paths().to_vec();
        assert!(!file_paths.is_empty());

        let project = db.project().expect("project should exist");
        let file_revisions: Vec<_> = file_paths
            .iter()
            .map(|path| {
                let file = path_to_file(&db, path).expect("fixture file should exist");
                (path.clone(), file.revision(&db))
            })
            .collect();
        assert!(!file_revisions.is_empty());

        let root_revisions: Vec<_> = project
            .search_paths(&db)
            .iter()
            .filter_map(|search_path| db.files().root(&db, search_path.path()))
            .map(|root| (root.path(&db).clone(), root.revision(&db)))
            .collect();
        assert!(!root_revisions.is_empty());

        let environment = djls_project::testing::compute_django_environment(&db, project);
        djls_project::apply_django_environment(&mut db, environment);
        let _facts = djls_project::testing::compute_project_facts(&db, project);

        let unchanged_file_revisions: Vec<_> = file_paths
            .iter()
            .map(|path| {
                let file = path_to_file(&db, path).expect("fixture file should exist");
                (path.clone(), file.revision(&db))
            })
            .collect();
        let unchanged_root_revisions: Vec<_> = project
            .search_paths(&db)
            .iter()
            .filter_map(|search_path| db.files().root(&db, search_path.path()))
            .map(|root| (root.path(&db).clone(), root.revision(&db)))
            .collect();

        assert_eq!(unchanged_file_revisions, file_revisions);
        assert!(unchanged_root_revisions.iter().zip(root_revisions).all(
            |((new_path, new_revision), (old_path, old_revision))| {
                new_path == &old_path && *new_revision > old_revision
            }
        ));
    }

    #[test]
    fn semantic_db_template_libraries_returns_derived_app_libraries() {
        let tempdir = tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf()).unwrap();
        std::fs::write(
            root.join("djls.toml").as_std_path(),
            "django_settings_module = \"settings\"\n",
        )
        .unwrap();
        let settings = Settings::new(root.as_path(), None).unwrap();

        let mut fs = InMemoryFileSystem::new();
        fs.add_file(root.join("manage.py"), String::new());
        fs.add_file(
            root.join("settings.py"),
            "INSTALLED_APPS = ['blog']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n".to_string(),
        );
        fs.add_file(root.join("blog/__init__.py"), String::new());
        fs.add_file(root.join("blog/templatetags/__init__.py"), String::new());
        fs.add_file(
            root.join("blog/templatetags/custom.py"),
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef hello():\n    pass\n".to_string(),
        );

        let mut db = DjangoDatabase {
            fs: Arc::new(fs),
            files: SourceFiles::default(),
            project: None,
            settings: Arc::new(Mutex::new(settings.clone())),
            storage: salsa::Storage::default(),
            logs: Arc::new(Mutex::new(None)),
        };
        let project = Project::bootstrap(&db, root.as_path(), &settings);
        db.project = Some(project);

        let djls_project::LoadableLibraryLookup::Found(custom) =
            db.template_libraries().loadable_library_str("custom")
        else {
            panic!("custom library should resolve definitively");
        };
        assert_eq!(custom.module_name_str(), "blog.templatetags.custom");
        assert!(
            custom
                .symbols()
                .iter()
                .any(|symbol| symbol.name() == "hello")
        );
    }

    #[test]
    fn templatetag_source_change_invalidates_template_libraries() {
        let event_log = EventLog::default();
        let tempdir = tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tempdir.path().to_path_buf()).unwrap();
        std::fs::write(
            root.join("djls.toml").as_std_path(),
            "django_settings_module = \"settings\"\n",
        )
        .unwrap();
        let settings = Settings::new(root.as_path(), None).unwrap();
        let tag_path = root.join("blog/templatetags/custom.py");

        let fs = Arc::new(Mutex::new(InMemoryFileSystem::new()));
        {
            let mut fs = fs.lock().unwrap();
            fs.add_file(root.join("manage.py"), String::new());
            fs.add_file(
                root.join("settings.py"),
                "INSTALLED_APPS = ['blog']\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': True}]\n".to_string(),
            );
            fs.add_file(root.join("blog/__init__.py"), String::new());
            fs.add_file(root.join("blog/templatetags/__init__.py"), String::new());
            fs.add_file(
                tag_path.clone(),
                "from django import template\nregister = template.Library()\n@register.simple_tag\ndef old_tag():\n    pass\n".to_string(),
            );
        }

        let mut db = DjangoDatabase {
            fs: fs.clone(),
            files: SourceFiles::default(),
            project: None,
            settings: Arc::new(Mutex::new(settings.clone())),
            storage: salsa::Storage::new(Some(Box::new({
                let log = event_log.clone();
                move |event| {
                    log.events.lock().unwrap().push(event);
                }
            }))),
            logs: Arc::new(Mutex::new(None)),
        };
        let project = Project::bootstrap(&db, root.as_path(), &settings);
        db.project = Some(project);

        let djls_project::LoadableLibraryLookup::Found(custom) =
            db.template_libraries().loadable_library_str("custom")
        else {
            panic!("custom library should resolve definitively");
        };
        assert!(
            custom
                .symbols()
                .iter()
                .any(|symbol| symbol.name() == "old_tag")
        );
        event_log.take();

        let tag_file = path_to_file(&db, &tag_path).expect("fixture file should exist");
        fs.lock().unwrap().add_file(
            tag_path,
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef new_tag():\n    pass\n".to_string(),
        );
        SourceChanges::new([ChangeEvent::ContentChanged(tag_file.path(&db).clone())])
            .apply(&mut db);

        let djls_project::LoadableLibraryLookup::Found(custom) =
            db.template_libraries().loadable_library_str("custom")
        else {
            panic!("custom library should resolve definitively");
        };
        assert!(
            custom
                .symbols()
                .iter()
                .any(|symbol| symbol.name() == "new_tag")
        );
        assert!(
            !custom
                .symbols()
                .iter()
                .any(|symbol| symbol.name() == "old_tag")
        );
        let events = event_log.take();
        assert!(
            was_executed(&db, &events, "template_libraries"),
            "template_libraries should re-execute after a templatetag source change"
        );
    }

    #[test]
    fn model_graph_empty_when_no_models() {
        let (db, _event_log) = test_db_with_project();
        let graph = db.model_graph();
        assert!(graph.is_empty());
    }

    #[test]
    fn model_graph_cached_on_repeated_access() {
        let (db, event_log) = test_db_with_project();

        let _graph1 = db.model_graph();
        let events = event_log.take();
        assert!(
            was_executed(&db, &events, "compute_model_graph"),
            "compute_model_graph should execute on first call"
        );

        let _graph2 = db.model_graph();
        let events = event_log.take();
        assert!(
            !was_executed(&db, &events, "compute_model_graph"),
            "compute_model_graph should NOT re-execute on second call (cached)"
        );
    }

    #[test]
    fn model_graph_reuses_unchanged_file_graphs_after_one_model_changes() {
        let event_log = EventLog::default();
        let settings = Settings::default();
        let root = Utf8PathBuf::from("/test/project");
        let first_model_path = root.join("app1/models.py");
        let second_model_path = root.join("app2/models.py");
        let fs = Arc::new(Mutex::new(InMemoryFileSystem::new()));
        {
            let mut fs = fs.lock().unwrap();
            fs.add_file(
                first_model_path.clone(),
                "from django.db import models\nclass First(models.Model):\n    pass\n".to_string(),
            );
            fs.add_file(
                second_model_path,
                "from django.db import models\nclass Second(models.Model):\n    pass\n".to_string(),
            );
        }

        let mut db = DjangoDatabase {
            fs: fs.clone(),
            files: SourceFiles::default(),
            project: None,
            settings: Arc::new(Mutex::new(settings.clone())),
            storage: salsa::Storage::new(Some(Box::new({
                let log = event_log.clone();
                move |event| {
                    log.events.lock().unwrap().push(event);
                }
            }))),
            logs: Arc::new(Mutex::new(None)),
        };
        let project = Project::bootstrap(&db, &root, &settings);
        db.project = Some(project);

        let graph = db.model_graph();
        assert!(graph.models_named("First").next().is_some());
        assert!(graph.models_named("Second").next().is_some());
        let events = event_log.take();
        let extracted_graph_count = events
            .iter()
            .filter(|event| match &event.kind {
                salsa::EventKind::WillExecute { database_key } => {
                    let name = db.ingredient_debug_name(database_key.ingredient_index());
                    name.contains("extract_models")
                }
                _ => false,
            })
            .count();
        assert_eq!(
            extracted_graph_count, 2,
            "both model files should be extracted on first model graph computation"
        );

        let first_model = path_to_file(&db, &first_model_path).expect("fixture file should exist");
        fs.lock().unwrap().add_file(
            first_model_path,
            "from django.db import models\nclass FirstRenamed(models.Model):\n    pass\n"
                .to_string(),
        );
        SourceChanges::new([ChangeEvent::ContentChanged(first_model.path(&db).clone())])
            .apply(&mut db);

        let graph = db.model_graph();
        assert!(graph.models_named("First").next().is_none());
        assert!(graph.models_named("FirstRenamed").next().is_some());
        assert!(graph.models_named("Second").next().is_some());
        let events = event_log.take();
        assert!(
            was_executed(&db, &events, "compute_model_graph"),
            "the aggregate model graph should re-run after one model file changes"
        );
        let extracted_graph_count = events
            .iter()
            .filter(|event| match &event.kind {
                salsa::EventKind::WillExecute { database_key } => {
                    let name = db.ingredient_debug_name(database_key.ingredient_index());
                    name.contains("extract_models")
                }
                _ => false,
            })
            .count();
        assert_eq!(
            extracted_graph_count, 1,
            "only the changed model file should re-run extraction"
        );
    }

    #[test]
    fn template_inheritance_reuses_chain_after_child_body_edit_keeps_symbols() {
        let TemplateInheritanceFixture {
            mut db,
            event_log,
            fs,
            project,
            child_file,
            parent_file: _,
            other_file: _,
            child_path,
            ..
        } = template_inheritance_fixture();

        let inheritance = template_inheritance(&db, project, child_file);
        assert_eq!(inheritance.ancestors(&db).len(), 1);
        event_log.take();

        fs.lock().unwrap().add_file(
            child_path,
            "{% extends \"base.html\" %}\n{% block content %}{{ two }}{% endblock %}".to_string(),
        );
        SourceChanges::new([ChangeEvent::ContentChanged(child_file.path(&db).clone())])
            .apply(&mut db);

        let nodelist = parse_template(&db, child_file).expect("child should parse");
        let symbols = template_symbols(&db, nodelist);
        assert_eq!(symbols.blocks()[0].name, "content");
        let inheritance = template_inheritance(&db, project, child_file);
        assert_eq!(inheritance.ancestors(&db).len(), 1);
        let events = event_log.take();
        assert_eq!(
            execution_count(&db, &events, "template_symbols"),
            1,
            "only the edited child template should recompute symbols"
        );
        assert!(
            !was_executed(&db, &events, "template_inheritance"),
            "unchanged child symbols should backdate before the chain re-runs"
        );
    }

    #[test]
    fn template_inheritance_reexecutes_after_child_extends_target_edit() {
        let TemplateInheritanceFixture {
            mut db,
            event_log,
            fs,
            project,
            child_file,
            other_file,
            child_path,
            ..
        } = template_inheritance_fixture();

        let inheritance = template_inheritance(&db, project, child_file);
        assert_eq!(inheritance.ancestors(&db).len(), 1);
        event_log.take();

        fs.lock().unwrap().add_file(
            child_path,
            "{% extends \"next.html\" %}\n{% block content %}{{ one }}{% endblock %}".to_string(),
        );
        SourceChanges::new([ChangeEvent::ContentChanged(child_file.path(&db).clone())])
            .apply(&mut db);

        let inheritance = template_inheritance(&db, project, child_file);
        assert!(inheritance.ancestors(&db)[0].file(&db) == other_file);
        let events = event_log.take();
        assert!(
            was_executed(&db, &events, "template_inheritance"),
            "changed extends target should re-run the chain"
        );
    }

    #[test]
    fn template_inheritance_reuses_child_chain_after_parent_block_name_edit() {
        let TemplateInheritanceFixture {
            mut db,
            event_log,
            fs,
            project,
            child_file,
            parent_file,
            parent_path,
            ..
        } = template_inheritance_fixture();

        let inheritance = template_inheritance(&db, project, child_file);
        assert!(inheritance.ancestors(&db)[0].file(&db) == parent_file);
        event_log.take();

        fs.lock().unwrap().add_file(
            parent_path,
            "{% block sidebar %}Base{% endblock %}".to_string(),
        );
        SourceChanges::new([ChangeEvent::ContentChanged(parent_file.path(&db).clone())])
            .apply(&mut db);

        let inheritance = template_inheritance(&db, project, child_file);
        assert!(inheritance.ancestors(&db)[0].file(&db) == parent_file);
        let events = event_log.take();
        assert_eq!(
            execution_count(&db, &events, "template_symbols"),
            1,
            "only the edited parent template should recompute symbols"
        );
        assert!(
            !was_executed(&db, &events, "template_inheritance"),
            "parent block names should not invalidate the child's inheritance chain"
        );
    }
}

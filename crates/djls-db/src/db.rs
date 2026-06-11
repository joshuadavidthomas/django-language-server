//! Concrete Salsa database implementation for the Django Language Server.
//!
//! This module provides the concrete [`DjangoDatabase`] that implements all
//! the database traits from source, semantic, and project crates. This follows
//! Ruff's architecture pattern where the concrete database lives at the top level.

use std::sync::Arc;
use std::sync::Mutex;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::Settings;
use djls_semantic::Db as SemanticDb;
use djls_semantic::Project;
use djls_semantic::ProjectDb;
use djls_semantic::ProjectIntrospector;
use djls_semantic::TagSpecs;
use djls_semantic::TemplateLibraries;
use djls_semantic::compute_filter_arity_specs;
use djls_semantic::compute_model_graph;
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
    pub(crate) fs: Arc<dyn FileSystem>,

    /// Registry of tracked files used by the workspace layer.
    pub(crate) files: SourceFiles,

    /// The single project for this database instance.
    ///
    /// This handle must remain stable for the lifetime of the database:
    /// tracked queries branch on the untracked `db.project()` read, so
    /// replacing the handle (or flipping None→Some after queries have run)
    /// changes results outside Salsa's dependency graph. Set once during
    /// construction; reloads mutate fields via Salsa setters
    /// (see `update_project_from_settings`). Same invariant as ty's
    /// `ProjectDatabase` (`ty_project/src/db.rs`).
    pub(crate) project: Option<Project>,

    /// Configuration settings for the language server
    pub(crate) settings: Arc<Mutex<Settings>>,

    /// Shared introspector for external project facts.
    pub(crate) project_introspector: Arc<ProjectIntrospector>,

    pub(crate) storage: salsa::Storage<Self>,

    // The logs are only used for testing and demonstrating reuse:
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) logs: Arc<Mutex<Option<Vec<String>>>>,
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
            project_introspector: Arc::new(ProjectIntrospector::new()),
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
            project_introspector: Arc::new(ProjectIntrospector::new()),
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
        let project = Project::bootstrap(self, root, settings);
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

    fn template_dirs(&self) -> Option<Vec<Utf8PathBuf>> {
        self.project().and_then(|project| {
            let (dirs, knowledge) = djls_semantic::template_dirs(self, project);
            (*knowledge == djls_semantic::StaticKnowledge::Known).then(|| dirs.clone())
        })
    }

    fn diagnostics_config(&self) -> djls_conf::DiagnosticsConfig {
        self.settings().diagnostics().clone()
    }

    fn template_libraries(&self) -> &TemplateLibraries {
        self.project()
            .map_or(TemplateLibraries::empty_ref(), |project| {
                djls_semantic::template_libraries(self, project)
            })
    }

    fn filter_arity_specs(&self) -> &djls_semantic::FilterAritySpecs {
        self.project()
            .map_or(djls_semantic::FilterAritySpecs::empty_ref(), |project| {
                compute_filter_arity_specs(self, project)
            })
    }

    fn model_graph(&self) -> &djls_semantic::ModelGraph {
        self.project()
            .map_or(djls_semantic::ModelGraph::empty_ref(), |project| {
                compute_model_graph(self, project)
            })
    }
}

#[salsa::db]
impl ProjectDb for DjangoDatabase {
    fn project(&self) -> Option<Project> {
        self.project
    }

    fn project_introspector(&self) -> Arc<ProjectIntrospector> {
        self.project_introspector.clone()
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
    use std::sync::Arc;
    use std::sync::Mutex;

    use camino::Utf8Path;
    use camino::Utf8PathBuf;
    use djls_conf::Settings;
    use djls_semantic::Db as SemanticDb;
    use djls_semantic::Project;
    use djls_semantic::SemanticOffsetContext;
    use djls_source::Db as SourceDb;
    use djls_source::InMemoryFileSystem;
    use djls_source::Offset;
    use djls_source::SourceFiles;
    use salsa::Database;
    use salsa::Setter;
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

    /// Create a test database with event logging and a pre-configured project.
    ///
    /// Uses `Interpreter::discover(None)` to match what `update_project_from_settings`
    /// produces, avoiding spurious interpreter mismatches from `$VIRTUAL_ENV`.
    fn test_db_with_project() -> (DjangoDatabase, EventLog) {
        let event_log = EventLog::default();
        let settings = Settings::default();

        let mut db = DjangoDatabase {
            fs: Arc::new(InMemoryFileSystem::new()),
            files: SourceFiles::default(),
            project: None,
            settings: Arc::new(Mutex::new(settings.clone())),
            project_introspector: Arc::new(djls_semantic::ProjectIntrospector::new()),
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
    fn settings_source_change_validates_templatetag_module_projection() {
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
            project_introspector: Arc::new(djls_semantic::ProjectIntrospector::new()),
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

        let _specs = db.tag_specs();
        event_log.take();

        let settings_file = db.get_or_create_file(&settings_path);
        fs.lock().unwrap().add_file(
            settings_path,
            "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'builtins': []}}]\n".to_string(),
        );
        db.bump_file_revision(settings_file);

        let _specs = db.tag_specs();
        let events = event_log.take();
        assert!(
            was_executed(&db, &events, "template_libraries"),
            "template_libraries should re-execute after the settings source changes"
        );
        assert!(
            was_executed(&db, &events, "templatetag_modules"),
            "templatetag_modules should re-execute after derived libraries change"
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
            project_introspector: Arc::new(djls_semantic::ProjectIntrospector::new()),
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
        let child = db.get_or_create_file(child_path);
        let offset = Offset::try_from(source.find("base.html").unwrap()).unwrap();
        {
            let SemanticOffsetContext::TemplateReference { name, .. } =
                SemanticOffsetContext::from_offset(&db, child, offset)
            else {
                panic!("expected extends argument to be a template reference");
            };
            let _result = djls_semantic::find_template(&db, project, name);
        }
        event_log.take();

        let root = db.files().expect_root(&db, child_path);
        db.bump_file_root_revision(root);

        let SemanticOffsetContext::TemplateReference { name, .. } =
            SemanticOffsetContext::from_offset(&db, child, offset)
        else {
            panic!("expected extends argument to be a template reference");
        };
        let _result = djls_semantic::find_template(&db, project, name);
        let events = event_log.take();
        assert!(
            was_executed(&db, &events, "project_template_files"),
            "project_template_files should re-execute after the search root revision changes"
        );
    }

    #[test]
    fn semantic_db_template_dirs_returns_none_when_derivation_is_unknown() {
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
            project_introspector: Arc::new(djls_semantic::ProjectIntrospector::new()),
            storage: salsa::Storage::default(),
            logs: Arc::new(Mutex::new(None)),
        };
        let project = Project::bootstrap(&db, root.as_path(), &settings);
        db.project = Some(project);

        assert!(db.template_dirs().is_none());
    }

    #[test]
    fn settings_source_change_invalidates_template_dirs() {
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
            project_introspector: Arc::new(djls_semantic::ProjectIntrospector::new()),
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

        assert_eq!(db.template_dirs().unwrap(), vec![templates_dir.clone()]);
        event_log.take();

        let settings_file = db.get_or_create_file(&settings_path);
        fs.lock().unwrap().add_file(
            settings_path,
            format!(
                "INSTALLED_APPS = []\nTEMPLATES = [{{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['{other_templates_dir}'], 'APP_DIRS': False}}]\n"
            ),
        );
        db.bump_file_revision(settings_file);

        assert_eq!(db.template_dirs().unwrap(), vec![other_templates_dir]);
        let events = event_log.take();
        assert!(
            was_executed(&db, &events, "template_dirs"),
            "template_dirs should re-execute after the settings source changes"
        );
    }

    #[test]
    fn unrelated_file_revision_does_not_invalidate_template_dirs() {
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
            project_introspector: Arc::new(djls_semantic::ProjectIntrospector::new()),
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

        assert_eq!(db.template_dirs().unwrap(), vec![templates_dir]);
        event_log.take();

        let other_file = db.get_or_create_file(&other_path);
        db.bump_file_revision(other_file);

        assert_eq!(db.template_dirs().unwrap(), vec![root.join("templates")]);
        let events = event_log.take();
        assert!(
            !was_executed(&db, &events, "template_dirs"),
            "template_dirs should stay cached after an unrelated file revision changes"
        );
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
    fn update_project_from_settings_unchanged_no_invalidation() {
        let (mut db, event_log) = test_db_with_project();

        // Prime the cache
        let _specs = db.tag_specs();
        event_log.take();

        // Call update_project_from_settings with default settings (same as project was created with)
        let settings = Settings::default();
        let env_changed = db.update_project_from_settings(&settings);
        assert!(
            !env_changed,
            "env should not have changed with default settings"
        );

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
        let file = djls_source::Db::get_or_create_file(
            &db,
            camino::Utf8Path::new("/test/project/tags.py"),
        );

        // First extraction
        let _result1 = djls_semantic::extract_filter_arities(
            &db,
            file,
            djls_semantic::ModulePath::new("test.project.tags"),
        );
        let events = event_log.take();
        assert!(
            was_executed(&db, &events, "extract_filter_arities"),
            "extract_filter_arities should execute on first call"
        );

        // Second call — cached
        let _result2 = djls_semantic::extract_filter_arities(
            &db,
            file,
            djls_semantic::ModulePath::new("test.project.tags"),
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
        let file = djls_source::Db::get_or_create_file(
            &db,
            camino::Utf8Path::new("/test/project/tags.py"),
        );
        let _result = djls_semantic::extract_filter_arities(
            &db,
            file,
            djls_semantic::ModulePath::new("test.project.tags"),
        );
        event_log.take();

        // Bump the file revision — but the source is still empty (file not in FS)
        file.set_revision(&mut db).to(1);

        // Salsa's backdate optimization: file.source() returns the same empty text,
        // so extract_filter_arities does NOT re-execute (correct behavior)
        let _result = djls_semantic::extract_filter_arities(
            &db,
            file,
            djls_semantic::ModulePath::new("test.project.tags"),
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
            project_introspector: Arc::new(djls_semantic::ProjectIntrospector::new()),
            storage: salsa::Storage::new(Some(Box::new({
                let log = event_log.clone();
                move |event| {
                    log.events.lock().unwrap().push(event);
                }
            }))),
            logs: Arc::new(Mutex::new(None)),
        };

        let file = djls_source::Db::get_or_create_file(
            &db,
            camino::Utf8Path::new("/test/project/tags.py"),
        );
        let result = djls_semantic::extract_filter_arities(
            &db,
            file,
            djls_semantic::ModulePath::new("test.project.tags"),
        );

        // Should extract the filter
        let key = djls_semantic::SymbolKey::filter("test.project.tags", "my_filter");
        assert!(
            result.contains_key(&key),
            "should extract filter from file content"
        );
        assert!(result[&key].expects_arg);

        let other_module_result = djls_semantic::extract_filter_arities(
            &db,
            file,
            djls_semantic::ModulePath::new("other.project.tags"),
        );
        let other_key = djls_semantic::SymbolKey::filter("other.project.tags", "my_filter");
        assert!(other_module_result.contains_key(&other_key));
        assert!(!other_module_result.contains_key(&key));
    }

    fn settings_with_custom_library(module_path: &str) -> String {
        "INSTALLED_APPS = []\nTEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'custom': '__MODULE__'}}}]\n"
            .replace("__MODULE__", module_path)
    }

    fn template_tag_library_source(tag_name: &str) -> String {
        format!(
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef {tag_name}():\n    pass\n"
        )
    }

    fn assert_custom_library_module(db: &DjangoDatabase, module_path: &str) {
        assert_eq!(
            db.template_libraries()
                .best_loadable_library_str("custom")
                .unwrap()
                .module()
                .as_str(),
            module_path
        );
    }

    fn assert_refresh_updates_star_imported_settings_source(settings_source: &str) {
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
            project_introspector: Arc::new(djls_semantic::ProjectIntrospector::new()),
            storage: salsa::Storage::default(),
            logs: Arc::new(Mutex::new(None)),
        };
        let project = Project::bootstrap(&db, root.as_path(), &settings);
        db.project = Some(project);

        assert_custom_library_module(&db, "old_tags");

        fs.lock()
            .unwrap()
            .add_file(base_settings_path, settings_with_custom_library("new_tags"));
        djls_semantic::refresh_external_data(&mut db);

        assert_custom_library_module(&db, "new_tags");
    }

    #[test]
    fn refresh_external_data_reads_changed_star_imported_settings_source_for_template_libraries() {
        assert_refresh_updates_star_imported_settings_source("from .base import *\n");
    }

    #[test]
    fn refresh_external_data_reads_changed_try_star_imported_settings_source() {
        assert_refresh_updates_star_imported_settings_source(
            "try:\n    from .base import *\nexcept ImportError:\n    pass\n",
        );
    }

    #[test]
    fn refresh_external_data_reads_changed_conditionally_star_imported_settings_source() {
        assert_refresh_updates_star_imported_settings_source(
            "import os\nif os.environ.get(\"EXTRA\"):\n    from .base import *\nelse:\n    from .base import *\n",
        );
    }

    #[test]
    fn refresh_external_data_discovers_newly_star_imported_known_file() {
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
            project_introspector: Arc::new(djls_semantic::ProjectIntrospector::new()),
            storage: salsa::Storage::default(),
            logs: Arc::new(Mutex::new(None)),
        };
        let project = Project::bootstrap(&db, root.as_path(), &settings);
        db.project = Some(project);

        assert!(
            db.template_libraries()
                .best_loadable_library_str("custom")
                .is_none()
        );
        let extra_file = djls_source::Db::get_or_create_file(&db, &extra_settings_path);
        assert!(extra_file.source(&db).as_str().contains("old_tags"));

        {
            let mut fs = fs.lock().unwrap();
            fs.add_file(settings_path, "from .extra import *\n".to_string());
            fs.add_file(
                extra_settings_path,
                settings_with_custom_library("new_tags"),
            );
        }
        djls_semantic::refresh_external_data(&mut db);

        assert_custom_library_module(&db, "new_tags");
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
            project_introspector: Arc::new(djls_semantic::ProjectIntrospector::new()),
            storage: salsa::Storage::default(),
            logs: Arc::new(Mutex::new(None)),
        };
        let project = Project::bootstrap(&db, root.as_path(), &settings);
        db.project = Some(project);

        let libraries = db.template_libraries();
        let custom = libraries
            .best_loadable_library_str("custom")
            .expect("custom library should be derived");
        assert_eq!(custom.module().as_str(), "blog.templatetags.custom");
        assert!(custom.symbols.iter().any(|symbol| symbol.name() == "hello"));
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
            project_introspector: Arc::new(djls_semantic::ProjectIntrospector::new()),
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

        let libraries = db.template_libraries();
        let custom = libraries.best_loadable_library_str("custom").unwrap();
        assert!(
            custom
                .symbols
                .iter()
                .any(|symbol| symbol.name() == "old_tag")
        );
        event_log.take();

        let tag_file = db.get_or_create_file(&tag_path);
        fs.lock().unwrap().add_file(
            tag_path,
            "from django import template\nregister = template.Library()\n@register.simple_tag\ndef new_tag():\n    pass\n".to_string(),
        );
        db.bump_file_revision(tag_file);

        let libraries = db.template_libraries();
        let custom = libraries.best_loadable_library_str("custom").unwrap();
        assert!(
            custom
                .symbols
                .iter()
                .any(|symbol| symbol.name() == "new_tag")
        );
        assert!(
            !custom
                .symbols
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
}

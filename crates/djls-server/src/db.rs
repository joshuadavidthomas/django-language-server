//! Concrete Salsa database implementation for the Django Language Server.
//!
//! This module provides the concrete [`DjangoDatabase`] that implements all
//! the database traits from workspace, template, and project crates. This follows
//! Ruff's architecture pattern where the concrete database lives at the top level.

use std::sync::Arc;
use std::sync::Mutex;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use salsa::Setter;
use djls_conf::Settings;
use djls_project::template_dirs;
use djls_project::Db as ProjectDb;
use djls_project::Inspector;
use djls_project::Interpreter;
use djls_project::Project;
use djls_project::TemplateTags;
use djls_project::TemplatetagsRequest;
use djls_project::TemplatetagsResponse;
use djls_semantic::Db as SemanticDb;
use djls_semantic::TagIndex;
use djls_semantic::TagSpecs;
use djls_source::Db as SourceDb;
use djls_source::File;
use djls_source::FxDashMap;
use djls_templates::Db as TemplateDb;
use djls_workspace::Db as WorkspaceDb;
use djls_workspace::FileSystem;

/// Compute `TagSpecs` from a project's config document and inspector inventory.
///
/// This tracked function reads `project.tagspecs(db)` and
/// `project.inspector_inventory(db)` to establish Salsa dependencies,
/// then converts the config document to semantic specs via
/// `TagSpecs::from_config_def`. Does NOT read from `Arc<Mutex<Settings>>`.
#[salsa::tracked]
pub fn compute_tag_specs(db: &dyn SemanticDb, project: Project) -> TagSpecs {
    // Read both fields to establish Salsa dependencies, even if we don't
    // use inspector_inventory yet (M3+ will integrate load-scoped tags).
    let _inventory = project.inspector_inventory(db);
    let tagspec_def = project.tagspecs(db);

    TagSpecs::from_config_def(tagspec_def)
}

/// Compute `TagIndex` from the project's `TagSpecs`.
///
/// Depends on `compute_tag_specs` — automatic invalidation cascade ensures
/// the index is rebuilt whenever specs change.
#[salsa::tracked]
pub fn compute_tag_index(db: &dyn SemanticDb, project: Project) -> TagIndex<'_> {
    let specs = compute_tag_specs(db, project);
    TagIndex::from_tag_specs(db, &specs)
}

/// Concrete Salsa database for the Django Language Server.
///
/// This database implements all the traits from various crates:
/// - [`WorkspaceDb`] for file system access and core operations
/// - [`TemplateDb`] for template parsing and diagnostics
/// - [`ProjectDb`] for project metadata and Python environment
#[salsa::db]
#[derive(Clone)]
pub struct DjangoDatabase {
    /// File system for reading file content (checks buffers first, then disk).
    fs: Arc<dyn FileSystem>,

    /// Registry of tracked files used by the workspace layer.
    files: Arc<FxDashMap<Utf8PathBuf, File>>,

    /// The single project for this database instance
    project: Arc<Mutex<Option<Project>>>,

    /// Configuration settings for the language server
    settings: Arc<Mutex<Settings>>,

    /// Shared inspector for executing Python queries
    inspector: Arc<Inspector>,

    storage: salsa::Storage<Self>,

    // The logs are only used for testing and demonstrating reuse:
    #[cfg(test)]
    #[allow(dead_code)]
    logs: Arc<Mutex<Option<Vec<String>>>>,
}

#[cfg(test)]
impl Default for DjangoDatabase {
    fn default() -> Self {
        use djls_workspace::InMemoryFileSystem;

        let logs = <Arc<Mutex<Option<Vec<String>>>>>::default();

        Self {
            fs: Arc::new(InMemoryFileSystem::new()),
            files: Arc::new(FxDashMap::default()),
            project: Arc::new(Mutex::new(None)),
            settings: Arc::new(Mutex::new(Settings::default())),
            inspector: Arc::new(Inspector::new()),
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
            files: Arc::new(FxDashMap::default()),
            project: Arc::new(Mutex::new(None)),
            settings: Arc::new(Mutex::new(settings.clone())),
            inspector: Arc::new(Inspector::new()),
            storage: salsa::Storage::new(None),
            #[cfg(test)]
            logs: Arc::new(Mutex::new(None)),
        };

        if let Some(path) = project_path {
            db.set_project(path, settings);
        }

        db
    }

    fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    /// Update the settings, updating the existing project's fields via manual
    /// comparison (Ruff/RA pattern) to avoid unnecessary Salsa invalidation.
    ///
    /// When a project exists, delegates to [`update_project_from_settings`] to
    /// surgically update only the fields that changed, keeping project identity
    /// stable. When no project exists, the settings are stored for future use.
    ///
    /// # Panics
    ///
    /// Panics if the settings mutex is poisoned (another thread panicked while holding the lock)
    pub fn set_settings(&mut self, settings: Settings) {
        *self.settings.lock().unwrap() = settings;

        if self.project().is_some() {
            let settings = self.settings();
            let env_changed = self.update_project_from_settings(&settings);
            if env_changed {
                self.refresh_inspector();
            }
        }
    }

    /// Update an existing project's fields from new settings, only calling
    /// Salsa setters when values actually change (Ruff/RA pattern).
    ///
    /// Returns `true` if environment-related fields changed (`interpreter`,
    /// `django_settings_module`, `pythonpath`), indicating the inspector should
    /// be refreshed.
    pub fn update_project_from_settings(&mut self, settings: &Settings) -> bool {
        let Some(project) = self.project() else {
            return false;
        };

        let mut env_changed = false;

        let new_interpreter = Interpreter::discover(settings.venv_path());
        if project.interpreter(self) != &new_interpreter {
            project.set_interpreter(self).to(new_interpreter);
            env_changed = true;
        }

        let new_dsm = settings
            .django_settings_module()
            .map(String::from)
            .or_else(|| {
                std::env::var("DJANGO_SETTINGS_MODULE")
                    .ok()
                    .filter(|s| !s.is_empty())
            });
        if project.django_settings_module(self) != &new_dsm {
            project.set_django_settings_module(self).to(new_dsm);
            env_changed = true;
        }

        let new_pythonpath = settings.pythonpath().to_vec();
        if project.pythonpath(self) != &new_pythonpath {
            project.set_pythonpath(self).to(new_pythonpath);
            env_changed = true;
        }

        let new_tagspecs = settings.tagspecs().clone();
        if project.tagspecs(self) != &new_tagspecs {
            project.set_tagspecs(self).to(new_tagspecs);
        }

        let new_diagnostics = settings.diagnostics().clone();
        if project.diagnostics(self) != &new_diagnostics {
            project.set_diagnostics(self).to(new_diagnostics);
        }

        env_changed
    }

    /// Query the Python inspector directly and update the project's inventory
    /// if the result differs from the current value.
    ///
    /// This is a side-effect operation that bypasses Salsa tracked functions,
    /// querying the inspector subprocess directly and only calling the Salsa
    /// setter when the inventory has actually changed (Ruff/RA pattern).
    pub fn refresh_inspector(&mut self) {
        let Some(project) = self.project() else {
            return;
        };

        let interpreter = project.interpreter(self).clone();
        let root = project.root(self).clone();
        let dsm = project.django_settings_module(self).clone();
        let pythonpath = project.pythonpath(self).clone();

        let new_inventory = match self.inspector.query::<TemplatetagsRequest, TemplatetagsResponse>(
            &interpreter,
            &root,
            dsm.as_deref(),
            &pythonpath,
            &TemplatetagsRequest,
        ) {
            Ok(response) if response.ok => {
                response.data.map(TemplateTags::from_response)
            }
            Ok(response) => {
                tracing::warn!(
                    "refresh_inspector: inspector returned ok=false, error={:?}",
                    response.error
                );
                None
            }
            Err(e) => {
                tracing::error!("refresh_inspector: inspector query failed: {}", e);
                None
            }
        };

        if project.inspector_inventory(self) != &new_inventory {
            project
                .set_inspector_inventory(self)
                .to(new_inventory);
        }
    }

    fn set_project(&mut self, root: &Utf8Path, settings: &Settings) {
        let project = Project::bootstrap(self, root, settings);
        *self.project.lock().unwrap() = Some(project);
    }
}

#[salsa::db]
impl salsa::Database for DjangoDatabase {}

#[salsa::db]
impl SourceDb for DjangoDatabase {
    fn create_file(&self, path: &Utf8Path) -> File {
        let file = File::new(self, path.to_owned(), 0);
        self.files.insert(path.to_owned(), file);
        file
    }

    fn get_file(&self, path: &Utf8Path) -> Option<File> {
        self.files.get(path).map(|entry| *entry)
    }

    fn read_file(&self, path: &Utf8Path) -> std::io::Result<String> {
        self.fs.read_to_string(path)
    }
}

#[salsa::db]
impl WorkspaceDb for DjangoDatabase {
    fn fs(&self) -> Arc<dyn FileSystem> {
        self.fs.clone()
    }
}

#[salsa::db]
impl TemplateDb for DjangoDatabase {}

#[salsa::db]
impl SemanticDb for DjangoDatabase {
    fn tag_specs(&self) -> TagSpecs {
        if let Some(project) = self.project() {
            compute_tag_specs(self, project)
        } else {
            djls_semantic::django_builtin_specs()
        }
    }

    fn tag_index(&self) -> TagIndex<'_> {
        if let Some(project) = self.project() {
            compute_tag_index(self, project)
        } else {
            TagIndex::from_specs(self)
        }
    }

    fn template_dirs(&self) -> Option<Vec<Utf8PathBuf>> {
        if let Some(project) = self.project() {
            template_dirs(self, project)
        } else {
            None
        }
    }

    fn diagnostics_config(&self) -> djls_conf::DiagnosticsConfig {
        if let Some(project) = self.project() {
            project.diagnostics(self).clone()
        } else {
            djls_conf::DiagnosticsConfig::default()
        }
    }

    fn inspector_inventory(&self) -> Option<TemplateTags> {
        self.project()
            .and_then(|project| project.inspector_inventory(self).clone())
    }
}

#[salsa::db]
impl ProjectDb for DjangoDatabase {
    fn project(&self) -> Option<Project> {
        *self.project.lock().unwrap()
    }

    fn inspector(&self) -> Arc<Inspector> {
        self.inspector.clone()
    }
}

#[cfg(test)]
mod invalidation_tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::Mutex;

    use djls_conf::Settings;
    use djls_conf::TagSpecDef;
    use djls_project::Interpreter;
    use djls_project::Project;
    use djls_project::TemplateTags;
    use djls_semantic::Db as SemanticDb;
    use djls_source::FxDashMap;
    use djls_workspace::InMemoryFileSystem;
    use salsa::Database;
    use salsa::Setter;

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

        let db = DjangoDatabase {
            fs: Arc::new(InMemoryFileSystem::new()),
            files: Arc::new(FxDashMap::default()),
            project: Arc::new(Mutex::new(None)),
            settings: Arc::new(Mutex::new(settings.clone())),
            inspector: Arc::new(djls_project::Inspector::new()),
            storage: salsa::Storage::new(Some(Box::new({
                let log = event_log.clone();
                move |event| {
                    log.events.lock().unwrap().push(event);
                }
            }))),
            logs: Arc::new(Mutex::new(None)),
        };

        let interpreter = Interpreter::discover(settings.venv_path());
        let dsm = settings
            .django_settings_module()
            .map(String::from)
            .or_else(|| {
                std::env::var("DJANGO_SETTINGS_MODULE")
                    .ok()
                    .filter(|s| !s.is_empty())
            });

        let project = Project::new(
            &db,
            "/test/project".into(),
            interpreter,
            dsm,
            settings.pythonpath().to_vec(),
            None,
            settings.tagspecs().clone(),
            settings.diagnostics().clone(),
        );
        *db.project.lock().unwrap() = Some(project);

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
    fn tagspecs_change_invalidates_compute_tag_specs() {
        let (mut db, event_log) = test_db_with_project();

        // Prime the cache
        let _specs = db.tag_specs();
        event_log.take();

        // Update tagspecs on the project with a real tag
        let project = db.project.lock().unwrap().unwrap();
        project
            .set_tagspecs(&mut db)
            .to(tagspec_with_custom_tag("newtag"));

        // Access again — should re-execute
        let _specs = db.tag_specs();
        let events = event_log.take();
        assert!(
            was_executed(&db, &events, "compute_tag_specs"),
            "compute_tag_specs should re-execute after tagspecs change"
        );
    }

    #[test]
    fn inspector_inventory_change_invalidates_compute_tag_specs() {
        let (mut db, event_log) = test_db_with_project();

        // Prime the cache
        let _specs = db.tag_specs();
        event_log.take();

        // Update inspector_inventory on the project
        let project = db.project.lock().unwrap().unwrap();
        let new_inventory = Some(TemplateTags::new(vec![], vec![], HashMap::default(), vec![]));
        project
            .set_inspector_inventory(&mut db)
            .to(new_inventory);

        // Access again — should re-execute
        let _specs = db.tag_specs();
        let events = event_log.take();
        assert!(
            was_executed(&db, &events, "compute_tag_specs"),
            "compute_tag_specs should re-execute after inspector_inventory change"
        );
    }

    #[test]
    fn same_value_no_invalidation() {
        let (db, event_log) = test_db_with_project();

        // Prime the cache
        let _specs = db.tag_specs();
        event_log.take();

        // "Update" tagspecs with an identical value — manual comparison
        // in update_project_from_settings prevents the setter call
        let project = db.project.lock().unwrap().unwrap();
        let current = project.tagspecs(&db).clone();

        // Simulate manual comparison: value is the same, so we don't call the setter
        assert_eq!(project.tagspecs(&db), &current);
        // No setter called — cache should be preserved

        let _specs = db.tag_specs();
        let events = event_log.take();
        assert!(
            !was_executed(&db, &events, "compute_tag_specs"),
            "compute_tag_specs should NOT re-execute when value is unchanged"
        );
    }

    /// Build a `TagSpecDef` containing a custom standalone tag, causing
    /// `TagSpecs::from_config_def` to produce a different result from the default.
    fn tagspec_with_custom_tag(tag_name: &str) -> TagSpecDef {
        TagSpecDef {
            version: "0.6.0".to_string(),
            engine: "django".to_string(),
            requires_engine: None,
            extends: vec![],
            libraries: vec![djls_conf::TagLibraryDef {
                module: "test.custom".to_string(),
                requires_engine: None,
                tags: vec![djls_conf::TagDef {
                    name: tag_name.to_string(),
                    tag_type: djls_conf::TagTypeDef::Standalone,
                    end: None,
                    intermediates: vec![],
                    args: vec![],
                    extra: None,
                }],
                extra: None,
            }],
            extra: None,
        }
    }

    #[test]
    fn tag_index_depends_on_tag_specs() {
        let (mut db, event_log) = test_db_with_project();

        // Prime both caches
        let _specs = db.tag_specs();
        let _index = db.tag_index();
        event_log.take();

        // Change tagspecs with an actual tag so the computed TagSpecs differ
        let project = db.project.lock().unwrap().unwrap();
        project
            .set_tagspecs(&mut db)
            .to(tagspec_with_custom_tag("mytag"));

        // Access tag_index — both compute_tag_specs and compute_tag_index should re-execute
        let _index = db.tag_index();
        let events = event_log.take();
        assert!(
            was_executed(&db, &events, "compute_tag_specs"),
            "compute_tag_specs should re-execute after tagspecs change"
        );
        assert!(
            was_executed(&db, &events, "compute_tag_index"),
            "compute_tag_index should re-execute when tag specs produced different output"
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
        assert!(!env_changed, "env should not have changed with default settings");

        // Access tag_specs — should still be cached
        let _specs = db.tag_specs();
        let events = event_log.take();
        assert!(
            !was_executed(&db, &events, "compute_tag_specs"),
            "compute_tag_specs should NOT re-execute when settings are unchanged"
        );
    }
}

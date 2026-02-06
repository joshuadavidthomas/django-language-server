//! Concrete Salsa database implementation for the Django Language Server.
//!
//! This module provides the concrete [`DjangoDatabase`] that implements all
//! the database traits from workspace, template, and project crates. This follows
//! Ruff's architecture pattern where the concrete database lives at the top level.

use std::sync::Arc;
use std::sync::Mutex;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::Settings;
use salsa::Setter;
use djls_project::inspector_query;
use djls_project::template_dirs;
use djls_project::Db as ProjectDb;
use djls_project::InspectorInventory;
use djls_project::Inspector;
use djls_project::Interpreter;
use djls_project::Project;
use djls_project::TemplateInventoryRequest;
use djls_semantic::django_builtin_specs;
use djls_semantic::Db as SemanticDb;
use djls_semantic::TagIndex;
use djls_semantic::TagSpecs;
use djls_source::Db as SourceDb;
use djls_source::File;
use djls_source::FxDashMap;
use djls_templates::Db as TemplateDb;
use djls_workspace::Db as WorkspaceDb;
use djls_workspace::FileSystem;

/// Compute tag specifications from all sources.
///
/// This tracked query reads only from Salsa-tracked Project fields,
/// starts with `django_builtin_specs()`, then merges user-defined specs
/// from the project's `TagSpecDef` config document.
#[salsa::tracked]
fn compute_tag_specs(db: &dyn SemanticDb, project: Project) -> TagSpecs {
    let _inventory = project.inspector_inventory(db);
    let tagspecs_def = project.tagspecs(db);

    let mut specs = django_builtin_specs();

    let user_specs = TagSpecs::from_config_def(tagspecs_def);
    if !user_specs.is_empty() {
        specs.merge(user_specs);
    }

    specs
}

/// Build the tag index from computed tag specs.
///
/// Depends on `compute_tag_specs` — automatic invalidation cascade.
#[salsa::tracked]
fn compute_tag_index(db: &dyn SemanticDb, project: Project) -> TagIndex<'_> {
    let _specs = compute_tag_specs(db, project);
    TagIndex::from_specs(db)
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

    /// Update the settings, delegating field updates to `update_project_from_settings`.
    ///
    /// # Panics
    ///
    /// Panics if the settings mutex is poisoned (another thread panicked while holding the lock)
    pub fn set_settings(&mut self, settings: &Settings) {
        *self.settings.lock().unwrap() = settings.clone();

        if let Some(project) = self.project() {
            self.update_project_from_settings(project, settings);
        }
    }

    /// Initialize the project from scratch. Called once during startup.
    ///
    /// Creates a new `Project` via `bootstrap` and then refreshes the inspector
    /// to populate the initial inventory.
    fn set_project(&mut self, root: &Utf8Path, settings: &Settings) {
        let project = Project::bootstrap(
            self,
            root,
            settings.venv_path(),
            settings.django_settings_module(),
            settings.pythonpath(),
            settings,
        );
        *self.project.lock().unwrap() = Some(project);

        self.refresh_inspector();
    }

    /// Update Project fields from settings, comparing before calling setters.
    ///
    /// Only calls Salsa setters when values actually changed (Ruff/RA pattern
    /// to avoid unnecessary invalidation). Triggers `refresh_inspector()` if
    /// Python environment fields change.
    fn update_project_from_settings(&mut self, project: Project, settings: &Settings) {
        let mut env_changed = false;

        let new_interpreter = Interpreter::discover(settings.venv_path());
        if project.interpreter(self) != &new_interpreter {
            project.set_interpreter(self).to(new_interpreter);
            env_changed = true;
        }

        let new_dsm = settings.django_settings_module().map(String::from);
        if project.django_settings_module(self) != &new_dsm {
            project.set_django_settings_module(self).to(new_dsm);
            env_changed = true;
        }

        let new_pp = settings.pythonpath().to_vec();
        if project.pythonpath(self) != &new_pp {
            project.set_pythonpath(self).to(new_pp);
            env_changed = true;
        }

        let new_tagspecs = settings.tagspecs().clone();
        if project.tagspecs(self) != &new_tagspecs {
            tracing::debug!("Tagspecs config changed, updating Project");
            project.set_tagspecs(self).to(new_tagspecs);
        }

        let new_diagnostics = settings.diagnostics().clone();
        if project.diagnostics(self) != &new_diagnostics {
            tracing::debug!("Diagnostics config changed, updating Project");
            project.set_diagnostics(self).to(new_diagnostics);
        }

        if env_changed {
            tracing::debug!("Python environment changed, refreshing inspector");
            self.refresh_inspector();
        }
    }

    /// Refresh the inspector inventory by querying Python directly.
    ///
    /// This method:
    /// 1. Queries the Python inspector via a single unified query (tags + filters)
    /// 2. Compares the new inventory with the current one
    /// 3. Updates `Project.inspector_inventory` only if changed
    ///
    /// Call this when:
    /// - Project is first initialized
    /// - Python environment changes (venv, PYTHONPATH)
    /// - User explicitly requests refresh
    fn refresh_inspector(&mut self) {
        let Some(project) = self.project() else {
            tracing::warn!("Cannot refresh inspector: no project set");
            return;
        };

        let new_inventory: Option<InspectorInventory> =
            inspector_query(self, &TemplateInventoryRequest).map(|response| {
                InspectorInventory::new(
                    response.libraries,
                    response.builtins,
                    response.templatetags,
                    response.templatefilters,
                )
            });

        let current = project.inspector_inventory(self);
        if current == &new_inventory {
            tracing::trace!("Inspector inventory unchanged, skipping update");
        } else {
            tracing::debug!(
                "Inspector inventory changed: {} tags, {} filters -> {} tags, {} filters",
                current.as_ref().map_or(0, InspectorInventory::tag_count),
                current.as_ref().map_or(0, InspectorInventory::filter_count),
                new_inventory.as_ref().map_or(0, InspectorInventory::tag_count),
                new_inventory.as_ref().map_or(0, InspectorInventory::filter_count),
            );
            project
                .set_inspector_inventory(self)
                .to(new_inventory);
        }
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
            django_builtin_specs()
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

    fn inspector_inventory(&self) -> Option<InspectorInventory> {
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
    use djls_conf::TagLibraryDef;
    use djls_project::Inspector;
    use djls_project::Interpreter;
    use djls_project::Project;
    use djls_project::InspectorInventory;
    use djls_semantic::Db as SemanticDb;
    use djls_source::FxDashMap;
    use djls_workspace::InMemoryFileSystem;
    use salsa::Setter;

    use super::DjangoDatabase;

    #[derive(Clone, Default)]
    struct EventLogger {
        events: Arc<Mutex<Vec<salsa::Event>>>,
    }

    impl EventLogger {
        fn push(&self, event: salsa::Event) {
            self.events.lock().unwrap().push(event);
        }

        fn clear(&self) {
            self.events.lock().unwrap().clear();
        }

        fn was_executed(&self, db: &dyn salsa::Database, query_name: &str) -> bool {
            self.events.lock().unwrap().iter().any(|event| {
                matches!(
                    &event.kind,
                    salsa::EventKind::WillExecute { database_key }
                    if db.ingredient_debug_name(database_key.ingredient_index()) == query_name
                )
            })
        }
    }

    struct TestDatabase {
        db: DjangoDatabase,
        logger: EventLogger,
    }

    impl TestDatabase {
        fn with_project() -> Self {
            let logger = EventLogger::default();
            let settings = Settings::default();

            let db = DjangoDatabase {
                fs: Arc::new(InMemoryFileSystem::new()),
                files: Arc::new(FxDashMap::default()),
                project: Arc::new(Mutex::new(None)),
                settings: Arc::new(Mutex::new(settings.clone())),
                inspector: Arc::new(Inspector::new()),
                storage: salsa::Storage::new(Some(Box::new({
                    let logger = logger.clone();
                    move |event| {
                        logger.push(event);
                    }
                }))),
                logs: Arc::new(Mutex::new(None)),
            };

            let project = Project::new(
                &db,
                camino::Utf8PathBuf::from("/test/project"),
                Interpreter::Auto,
                Some("test.settings".to_string()),
                vec![],
                None,
                settings.tagspecs().clone(),
                settings.diagnostics().clone(),
            );
            *db.project.lock().unwrap() = Some(project);

            Self { db, logger }
        }
    }

    #[test]
    fn tag_specs_cached_on_repeated_access() {
        let test = TestDatabase::with_project();

        let _specs1 = test.db.tag_specs();
        assert!(
            test.logger.was_executed(&test.db, "compute_tag_specs"),
            "compute_tag_specs should execute on first access"
        );

        test.logger.clear();

        let _specs2 = test.db.tag_specs();
        assert!(
            !test.logger.was_executed(&test.db, "compute_tag_specs"),
            "compute_tag_specs should be cached on second access"
        );
    }

    #[test]
    fn tagspecs_change_invalidates() {
        let mut test = TestDatabase::with_project();

        let _specs1 = test.db.tag_specs();
        test.logger.clear();

        let project = test.db.project.lock().unwrap().expect("test project exists");
        let mut new_tagspecs = project.tagspecs(&test.db).clone();
        new_tagspecs.libraries.push(TagLibraryDef {
            module: "test.templatetags".to_string(),
            requires_engine: None,
            tags: vec![],
            extra: None,
        });
        project.set_tagspecs(&mut test.db).to(new_tagspecs);

        let _specs2 = test.db.tag_specs();
        assert!(
            test.logger.was_executed(&test.db, "compute_tag_specs"),
            "compute_tag_specs should recompute after tagspecs change"
        );
    }

    #[test]
    fn inspector_inventory_change_invalidates() {
        let mut test = TestDatabase::with_project();

        let _specs1 = test.db.tag_specs();
        test.logger.clear();

        let project = test.db.project.lock().unwrap().expect("test project exists");
        let new_inventory = InspectorInventory::new(
            HashMap::new(),
            vec!["django.template.defaulttags".to_string()],
            vec![],
            vec![],
        );
        project
            .set_inspector_inventory(&mut test.db)
            .to(Some(new_inventory));

        let _specs2 = test.db.tag_specs();
        assert!(
            test.logger.was_executed(&test.db, "compute_tag_specs"),
            "compute_tag_specs should recompute after inventory change"
        );
    }

    #[test]
    fn same_value_no_invalidation() {
        let test = TestDatabase::with_project();

        let _specs1 = test.db.tag_specs();
        test.logger.clear();

        let project = test.db.project.lock().unwrap().expect("test project exists");
        let same_tagspecs = project.tagspecs(&test.db).clone();

        // Manual comparison shows no change — don't call setter
        assert!(project.tagspecs(&test.db) == &same_tagspecs);

        let _specs2 = test.db.tag_specs();
        assert!(
            !test.logger.was_executed(&test.db, "compute_tag_specs"),
            "compute_tag_specs should NOT recompute when value unchanged"
        );
    }

    #[test]
    fn tag_index_depends_on_tag_specs() {
        let mut test = TestDatabase::with_project();

        let _index1 = test.db.tag_index();
        assert!(
            test.logger.was_executed(&test.db, "compute_tag_specs"),
            "tag_index should trigger tag_specs on first access"
        );

        test.logger.clear();

        let project = test.db.project.lock().unwrap().expect("test project exists");
        let mut new_tagspecs = project.tagspecs(&test.db).clone();
        new_tagspecs.libraries.push(TagLibraryDef {
            module: "another.templatetags".to_string(),
            requires_engine: None,
            tags: vec![],
            extra: None,
        });
        project.set_tagspecs(&mut test.db).to(new_tagspecs);

        let _index2 = test.db.tag_index();
        assert!(
            test.logger.was_executed(&test.db, "compute_tag_specs"),
            "tag_index should recompute when tagspecs change"
        );
    }
}

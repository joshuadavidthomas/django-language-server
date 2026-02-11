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
use djls_project::build_search_paths;
use djls_project::template_dirs;
use djls_project::Db as ProjectDb;
use djls_project::Inspector;
use djls_project::Interpreter;
use djls_project::Project;
use djls_project::TemplateLibraries;
use djls_project::TemplateLibrariesRequest;
use djls_project::TemplateLibrariesResponse;
use djls_project::TemplateLibrary;
use djls_semantic::Db as SemanticDb;
use djls_semantic::TagIndex;
use djls_semantic::TagSpecs;
use djls_source::Db as SourceDb;
use djls_source::File;
use djls_source::FxDashMap;
use djls_templates::Db as TemplateDb;
use djls_workspace::Db as WorkspaceDb;
use djls_workspace::FileSystem;
use salsa::Setter;

/// Compute `TagSpecs` from extraction results.
///
/// This tracked function reads `project.template_libraries(db)` and
/// `project.extracted_external_rules(db)` to establish Salsa dependencies.
/// It starts from empty specs and populates purely from extraction results
/// (both workspace modules via tracked queries and external modules from
/// the Project field).
///
/// Does NOT read from `Arc<Mutex<Settings>>`.
#[salsa::tracked]
pub fn compute_tag_specs(db: &dyn SemanticDb, project: Project) -> TagSpecs {
    let _libraries = project.template_libraries(db);
    let tagspecs = project.tagspecs(db);

    let mut specs = TagSpecs::default();

    // Merge workspace extraction results (tracked, auto-invalidating on file change)
    let workspace_results = collect_workspace_extraction_results(db, project);
    for (_module_path, extraction) in &workspace_results {
        specs.merge_extraction_results(extraction);
    }

    // Merge external extraction results (from Project field, updated by refresh_inspector)
    for extraction in project.extracted_external_rules(db).values() {
        specs.merge_extraction_results(extraction);
    }

    // Fill extraction gaps with manual TagSpecs configuration (fallback).
    // Extraction always wins.
    if !tagspecs.libraries.is_empty() {
        let fallback = TagSpecs::from_tagspec_def(tagspecs);
        specs.merge_fallback(fallback);
    }

    specs
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

/// Compute `FilterAritySpecs` from a project's extraction results.
///
/// Merges filter arity data from both workspace (tracked) and external
/// extraction results, with last-wins semantics for name collisions
/// (matching Django's builtin ordering).
#[salsa::tracked]
pub fn compute_filter_arity_specs(
    db: &dyn SemanticDb,
    project: Project,
) -> djls_semantic::FilterAritySpecs {
    let mut specs = djls_semantic::FilterAritySpecs::new();

    // Merge workspace extraction results (tracked)
    let workspace_results = collect_workspace_extraction_results(db, project);
    for (_module_path, extraction) in &workspace_results {
        specs.merge_extraction_result(extraction);
    }

    // Merge external extraction results (from Project field)
    for extraction in project.extracted_external_rules(db).values() {
        specs.merge_extraction_result(extraction);
    }

    specs
}

/// Extract validation rules from a Python registration module file.
///
/// Collect extracted rules from all workspace registration modules.
///
/// This tracked query:
/// 1. Gets registration modules from inspector inventory
/// 2. Resolves workspace modules to `File` inputs via `get_or_create_file`
/// 3. Extracts rules from each (via tracked `djls_python::extract_module`)
///
/// External modules are handled separately (cached on `Project` field,
/// updated by `refresh_inspector`). This function only processes workspace
/// modules, giving them automatic Salsa invalidation when files change.
#[salsa::tracked]
fn collect_workspace_extraction_results(
    db: &dyn SemanticDb,
    project: Project,
) -> Vec<(String, djls_python::ExtractionResult)> {
    let template_libraries = project.template_libraries(db);
    let interpreter = project.interpreter(db);
    let root = project.root(db);
    let pythonpath = project.pythonpath(db);

    let module_paths = template_libraries.registration_modules();
    if module_paths.is_empty() {
        return Vec::new();
    }

    let module_paths: Vec<String> = module_paths
        .iter()
        .map(|m| m.as_str().to_string())
        .collect();

    let search_paths = build_search_paths(interpreter, root, pythonpath);

    let (workspace_modules, _external) =
        djls_project::resolve_modules(module_paths.iter().map(String::as_str), &search_paths, root);

    let mut results = Vec::new();

    for resolved in workspace_modules {
        let file = db.get_or_create_file(&resolved.file_path);
        let mut extraction = djls_python::extract_module(db, file);

        if !extraction.is_empty() {
            extraction.rekey_module(&resolved.module_path);
            results.push((resolved.module_path, extraction));
        }
    }

    results
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SettingsUpdate {
    pub env_changed: bool,
    pub diagnostics_changed: bool,
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
    pub fn set_settings(&mut self, settings: Settings) -> SettingsUpdate {
        let previous = self.settings();
        *self.settings.lock().unwrap() = settings;

        let diagnostics_changed = previous.diagnostics() != self.settings().diagnostics();

        if self.project().is_some() {
            let settings = self.settings();
            let env_changed = self.update_project_from_settings(&settings);
            return SettingsUpdate {
                env_changed,
                diagnostics_changed,
            };
        }

        SettingsUpdate {
            env_changed: false,
            diagnostics_changed,
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

        env_changed
    }

    /// Refresh all inspector-derived data: inventory and external rules.
    ///
    /// This is a side-effect operation that bypasses Salsa tracked functions,
    /// querying the inspector subprocess directly and only calling Salsa
    /// setters when values have actually changed (Ruff/RA pattern).
    pub fn refresh_inspector(&mut self) {
        self.query_inspector_template_libraries();
        self.extract_external_rules();
    }

    /// Query the Python inspector subprocess and update the project's template libraries.
    fn query_inspector_template_libraries(&mut self) {
        let Some(project) = self.project() else {
            return;
        };

        let interpreter = project.interpreter(self).clone();
        let root = project.root(self).clone();
        let dsm = project.django_settings_module(self).clone();
        let pythonpath = project.pythonpath(self).clone();

        let response = match self
            .inspector
            .query::<TemplateLibrariesRequest, TemplateLibrariesResponse>(
                &interpreter,
                &root,
                dsm.as_deref(),
                &pythonpath,
                &TemplateLibrariesRequest,
            ) {
            Ok(response) if response.ok => response.data,
            Ok(response) => {
                tracing::warn!(
                    "query_inspector: inspector returned ok=false, error={:?}",
                    response.error
                );
                None
            }
            Err(e) => {
                tracing::error!("query_inspector: inspector query failed: {}", e);
                None
            }
        };

        let current = project.template_libraries(self).clone();
        let next = current.apply_inspector(response);
        if project.template_libraries(self) != &next {
            project.set_template_libraries(self).to(next);
        }
    }

    /// Extract validation rules from external (non-workspace) registration modules
    /// and update the project's extracted rules if they differ.
    ///
    /// Workspace modules are handled separately by `collect_workspace_extraction_results`
    /// which uses tracked Salsa queries for automatic invalidation on file change.
    fn extract_external_rules(&mut self) {
        let Some(project) = self.project() else {
            return;
        };

        let interpreter = project.interpreter(self).clone();
        let root = project.root(self).clone();
        let pythonpath = project.pythonpath(self).clone();

        let modules: rustc_hash::FxHashSet<String> = project
            .template_libraries(self)
            .registration_modules()
            .into_iter()
            .map(|m| m.as_str().to_string())
            .collect();

        let new_extraction = if modules.is_empty() {
            rustc_hash::FxHashMap::default()
        } else {
            djls_project::extract_external_rules(&modules, &interpreter, &root, &pythonpath)
        };

        if project.extracted_external_rules(self) != &new_extraction {
            project
                .set_extracted_external_rules(self)
                .to(new_extraction);
        }
    }

    /// Update the project's discovered template libraries.
    ///
    /// This is a side-effect operation that should be run off the LSP request path.
    /// It only calls Salsa setters when values have actually changed.
    pub fn update_discovered_template_libraries(&mut self, libraries: &[TemplateLibrary]) {
        let Some(project) = self.project() else {
            return;
        };

        let current = project.template_libraries(self).clone();
        let next = current.apply_discovery(libraries.iter().cloned());
        if project.template_libraries(self) != &next {
            project.set_template_libraries(self).to(next);
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
            TagSpecs::default()
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
        self.settings().diagnostics().clone()
    }

    fn template_libraries(&self) -> TemplateLibraries {
        self.project()
            .map(|project| project.template_libraries(self).clone())
            .unwrap_or_default()
    }

    fn filter_arity_specs(&self) -> djls_semantic::FilterAritySpecs {
        if let Some(project) = self.project() {
            compute_filter_arity_specs(self, project)
        } else {
            djls_semantic::FilterAritySpecs::new()
        }
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
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::Mutex;

    use djls_conf::Settings;
    use djls_project::Interpreter;
    use djls_project::Knowledge;
    use djls_project::LibraryName;
    use djls_project::LibraryOrigin;
    use djls_project::Project;
    use djls_project::PyModuleName;
    use djls_project::SymbolDefinition;
    use djls_project::TemplateLibraries;
    use djls_project::TemplateLibrary;
    use djls_project::TemplateSymbol;
    use djls_project::TemplateSymbolKind;
    use djls_project::TemplateSymbolName;
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
            settings.tagspecs().clone(),
            TemplateLibraries::default(),
            rustc_hash::FxHashMap::default(),
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
    fn template_libraries_change_invalidates_compute_tag_specs() {
        let (mut db, event_log) = test_db_with_project();

        // Prime the cache
        let _specs = db.tag_specs();
        event_log.take();

        // Update template_libraries on the project
        let project = db.project.lock().unwrap().unwrap();

        let response = djls_project::TemplateLibrariesResponse {
            symbols: Vec::new(),
            libraries: BTreeMap::new(),
            builtins: Vec::new(),
        };

        let new_libraries = TemplateLibraries::default().apply_inspector(Some(response));

        project.set_template_libraries(&mut db).to(new_libraries);

        // Access again — should re-execute
        let _specs = db.tag_specs();
        let events = event_log.take();
        assert!(
            was_executed(&db, &events, "compute_tag_specs"),
            "compute_tag_specs should re-execute after template_libraries change"
        );
    }

    #[test]
    fn tagspecs_change_invalidates_compute_tag_specs() {
        let (mut db, event_log) = test_db_with_project();

        // Prime the cache
        let _specs = db.tag_specs();
        event_log.take();

        let project = db.project.lock().unwrap().unwrap();

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
        let project = db.project.lock().unwrap().unwrap();
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
    fn tag_index_depends_on_tag_specs() {
        let (mut db, event_log) = test_db_with_project();

        // Prime both caches
        let _specs = db.tag_specs();
        let _index = db.tag_index();
        event_log.take();

        // Change extraction results to produce different TagSpecs
        let project = db.project.lock().unwrap().unwrap();
        let mut extraction = djls_python::ExtractionResult::default();
        extraction.block_specs.insert(
            djls_python::SymbolKey::tag("test.module", "mytag"),
            djls_python::BlockSpec {
                end_tag: Some("endmytag".to_string()),
                intermediates: vec![],
                opaque: false,
            },
        );
        let mut external_rules = rustc_hash::FxHashMap::default();
        external_rules.insert("test.module".to_string(), extraction);
        project
            .set_extracted_external_rules(&mut db)
            .to(external_rules);

        // Access tag_index — both compute_tag_specs and compute_tag_index should re-execute
        let _index = db.tag_index();
        let events = event_log.take();
        assert!(
            was_executed(&db, &events, "compute_tag_specs"),
            "compute_tag_specs should re-execute after extraction results change"
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
    fn extraction_result_cached_on_repeated_access() {
        let (db, event_log) = test_db_with_project();

        // Create a Python file and track it
        let file = djls_source::File::new(&db, "/test/project/tags.py".into(), 0);

        // First extraction
        let _result1 = djls_python::extract_module(&db, file);
        let events = event_log.take();
        assert!(
            was_executed(&db, &events, "extract_module"),
            "extract_module should execute on first call"
        );

        // Second call — cached
        let _result2 = djls_python::extract_module(&db, file);
        let events = event_log.take();
        assert!(
            !was_executed(&db, &events, "extract_module"),
            "extract_module should NOT re-execute on second call (cached)"
        );
    }

    #[test]
    fn file_revision_change_with_same_source_backdates() {
        let (mut db, event_log) = test_db_with_project();

        // Create and extract from a file (file doesn't exist, source is empty)
        let file = djls_source::File::new(&db, "/test/project/tags.py".into(), 0);
        let _result = djls_python::extract_module(&db, file);
        event_log.take();

        // Bump the file revision — but the source is still empty (file not in FS)
        file.set_revision(&mut db).to(1);

        // Salsa's backdate optimization: file.source() returns the same empty text,
        // so extract_module does NOT re-execute (correct behavior)
        let _result = djls_python::extract_module(&db, file);
        let events = event_log.take();
        assert!(
            !was_executed(&db, &events, "extract_module"),
            "extract_module should NOT re-execute when source content is unchanged (backdate)"
        );
    }

    #[test]
    fn file_with_different_content_produces_different_extraction() {
        use djls_workspace::InMemoryFileSystem;

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

        let file = djls_source::File::new(&db, "/test/project/tags.py".into(), 0);
        let result = djls_python::extract_module(&db, file);

        // Should extract the filter
        let key = djls_python::SymbolKey::filter("", "my_filter");
        assert!(
            result.filter_arities.contains_key(&key),
            "should extract filter from file content"
        );
        assert!(result.filter_arities[&key].expects_arg);
    }

    #[test]
    fn external_rules_stored_on_project() {
        let (mut db, _event_log) = test_db_with_project();
        let project = db.project.lock().unwrap().unwrap();

        // Initially empty
        assert!(
            project.extracted_external_rules(&db).is_empty(),
            "extracted_external_rules should be empty initially"
        );

        // Set some extraction results
        let mut extraction = djls_python::ExtractionResult::default();
        extraction.block_specs.insert(
            djls_python::SymbolKey::tag("test.module", "mytag"),
            djls_python::BlockSpec {
                end_tag: Some("endmytag".to_string()),
                intermediates: vec![],
                opaque: false,
            },
        );
        let mut external_rules = rustc_hash::FxHashMap::default();
        external_rules.insert("test.module".to_string(), extraction);
        project
            .set_extracted_external_rules(&mut db)
            .to(external_rules);

        let stored = project.extracted_external_rules(&db);
        assert_eq!(stored.len(), 1);
        assert_eq!(stored["test.module"].block_specs.len(), 1);
    }

    #[test]
    fn discovered_template_libraries_stored_on_project() {
        let (db, _event_log) = test_db_with_project();

        let project = db.project.lock().unwrap().unwrap();
        assert_eq!(
            project.template_libraries(&db).discovery_knowledge,
            Knowledge::Unknown
        );
        assert!(
            project.template_libraries(&db).loadable.is_empty(),
            "template libraries should initially be empty"
        );
    }

    #[test]
    fn discovered_template_libraries_setter_updates_value() {
        let (mut db, _event_log) = test_db_with_project();

        let project = db.project.lock().unwrap().unwrap();

        let name = LibraryName::parse("humanize").unwrap();
        let app_module = PyModuleName::parse("django.contrib.humanize").unwrap();
        let module = PyModuleName::parse("django.contrib.humanize.templatetags.humanize").unwrap();
        let source_path = camino::Utf8PathBuf::from(
            "/site-packages/django/contrib/humanize/templatetags/humanize.py",
        );

        let origin = LibraryOrigin {
            app: app_module,
            module,
            path: source_path.clone(),
        };

        let mut library = TemplateLibrary::new_discovered(name, origin);
        library.symbols = vec![
            TemplateSymbol {
                kind: TemplateSymbolKind::Filter,
                name: TemplateSymbolName::parse("intcomma").unwrap(),
                definition: SymbolDefinition::LibraryFile(source_path.clone()),
                doc: None,
            },
            TemplateSymbol {
                kind: TemplateSymbolKind::Filter,
                name: TemplateSymbolName::parse("intword").unwrap(),
                definition: SymbolDefinition::LibraryFile(source_path),
                doc: None,
            },
        ];

        let next = project
            .template_libraries(&db)
            .clone()
            .apply_discovery(vec![library]);
        project.set_template_libraries(&mut db).to(next);

        assert_eq!(
            project.template_libraries(&db).discovery_knowledge,
            Knowledge::Known
        );
        assert!(project
            .template_libraries(&db)
            .loadable
            .keys()
            .any(|k| k.as_str() == "humanize"));
    }

    #[test]
    fn template_libraries_same_value_no_invalidation() {
        let (mut db, event_log) = test_db_with_project();

        // Prime tag_specs cache
        let _specs = db.tag_specs();
        event_log.take();

        let project = db.project.lock().unwrap().unwrap();

        // Setting the same value should not trigger invalidation.
        // (manual comparison prevents setter call)
        let current = project.template_libraries(&db).clone();
        if project.template_libraries(&db) != &current {
            project.set_template_libraries(&mut db).to(current);
        }

        // tag_specs should NOT re-execute
        let _specs = db.tag_specs();
        let events = event_log.take();
        assert!(
            !was_executed(&db, &events, "compute_tag_specs"),
            "compute_tag_specs should not re-execute when template_libraries unchanged"
        );
    }

    #[test]
    fn extracted_rules_change_invalidates_compute_tag_specs() {
        let (mut db, event_log) = test_db_with_project();

        // Prime the cache
        let _specs = db.tag_specs();
        event_log.take();

        // Set extraction results on the project
        let project = db.project.lock().unwrap().unwrap();
        let mut extraction = djls_python::ExtractionResult::default();
        extraction.block_specs.insert(
            djls_python::SymbolKey::tag("test.module", "customblock"),
            djls_python::BlockSpec {
                end_tag: Some("endcustomblock".to_string()),
                intermediates: vec![],
                opaque: false,
            },
        );
        let mut external_rules = rustc_hash::FxHashMap::default();
        external_rules.insert("test.module".to_string(), extraction);
        project
            .set_extracted_external_rules(&mut db)
            .to(external_rules);

        // Access tag_specs — should re-execute because extracted_external_rules changed
        let specs = db.tag_specs();
        let events = event_log.take();
        assert!(
            was_executed(&db, &events, "compute_tag_specs"),
            "compute_tag_specs should re-execute after extracted_external_rules change"
        );

        // The merged specs should include the extracted block tag
        assert!(
            specs.get("customblock").is_some(),
            "tag specs should include extracted block tag"
        );
        assert_eq!(
            specs
                .get("customblock")
                .unwrap()
                .end_tag
                .as_ref()
                .unwrap()
                .name
                .as_ref(),
            "endcustomblock"
        );
    }
}

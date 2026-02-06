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
use salsa::Setter;

/// Compute `TagSpecs` from builtin specs and extraction results.
///
/// This tracked function reads `project.inspector_inventory(db)` and
/// `project.extracted_external_rules(db)` to establish Salsa dependencies.
/// It starts with `django_builtin_specs()` and merges extraction results
/// to enrich/override the handcoded defaults.
///
/// Does NOT read from `Arc<Mutex<Settings>>`.
#[salsa::tracked]
pub fn compute_tag_specs(db: &dyn SemanticDb, project: Project) -> TagSpecs {
    let _inventory = project.inspector_inventory(db);
    let extracted = project.extracted_external_rules(db);

    let mut specs = djls_semantic::django_builtin_specs();

    if let Some(extraction) = extracted {
        specs.merge_extraction_results(extraction);
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
/// Reads `project.extracted_external_rules(db)` to establish Salsa dependencies.
/// Merges all filter arity data from extraction results, with last-wins
/// semantics for name collisions (matching Django's builtin ordering).
#[salsa::tracked]
pub fn compute_filter_arity_specs(
    db: &dyn SemanticDb,
    project: Project,
) -> djls_semantic::FilterAritySpecs {
    let extracted = project.extracted_external_rules(db);

    let mut specs = djls_semantic::FilterAritySpecs::new();

    if let Some(extraction) = extracted {
        specs.merge_extraction_result(extraction);
    }

    specs
}

/// Extract validation rules from a Python registration module file.
///
/// This tracked function depends on `file.source(db)`, so editing the file
/// automatically invalidates the extraction result via Salsa's dependency
/// tracking.
///
/// The `ExtractionResult` uses empty `registration_module` in `SymbolKey`s.
/// Callers must re-key with the actual dotted module path when merging
/// results into `TagSpecs`.
#[salsa::tracked]
#[allow(dead_code)]
pub fn extract_module_rules(db: &dyn SemanticDb, file: File) -> djls_extraction::ExtractionResult {
    let source = file.source(db);
    djls_extraction::extract_rules(source.as_str(), "")
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

        let new_diagnostics = settings.diagnostics().clone();
        if project.diagnostics(self) != &new_diagnostics {
            project.set_diagnostics(self).to(new_diagnostics);
        }

        env_changed
    }

    /// Query the Python inspector directly and update the project's inventory
    /// and extraction results if they differ from the current values.
    ///
    /// This is a side-effect operation that bypasses Salsa tracked functions,
    /// querying the inspector subprocess directly and only calling the Salsa
    /// setter when the inventory has actually changed (Ruff/RA pattern).
    ///
    /// After updating the inventory, also extracts validation rules from the
    /// registration modules found in the inventory (external modules only;
    /// workspace files use tracked queries for automatic invalidation).
    pub fn refresh_inspector(&mut self) {
        let Some(project) = self.project() else {
            return;
        };

        let interpreter = project.interpreter(self).clone();
        let root = project.root(self).clone();
        let dsm = project.django_settings_module(self).clone();
        let pythonpath = project.pythonpath(self).clone();

        let new_inventory = match self
            .inspector
            .query::<TemplatetagsRequest, TemplatetagsResponse>(
                &interpreter,
                &root,
                dsm.as_deref(),
                &pythonpath,
                &TemplatetagsRequest,
            ) {
            Ok(response) if response.ok => response.data.map(TemplateTags::from_response),
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
                .to(new_inventory.clone());
        }

        // Extract rules from external registration modules
        let new_extraction = new_inventory
            .as_ref()
            .map(|inv| extract_external_rules(inv, &interpreter, &root, &pythonpath));

        if project.extracted_external_rules(self) != &new_extraction {
            project
                .set_extracted_external_rules(self)
                .to(new_extraction);
        }
    }

    fn set_project(&mut self, root: &Utf8Path, settings: &Settings) {
        let project = Project::bootstrap(self, root, settings);
        *self.project.lock().unwrap() = Some(project);
    }
}

/// Extract validation rules from all registration modules in the inventory.
///
/// Collects unique registration module paths from tags and filters,
/// resolves each to a file path, reads the source, and runs extraction.
fn extract_external_rules(
    inventory: &TemplateTags,
    interpreter: &Interpreter,
    root: &Utf8Path,
    pythonpath: &[String],
) -> djls_extraction::ExtractionResult {
    use rustc_hash::FxHashSet;

    let mut modules = FxHashSet::default();
    for tag in inventory.tags() {
        modules.insert(tag.registration_module().to_string());
    }
    for filter in inventory.filters() {
        let module = match filter.provenance() {
            djls_project::TagProvenance::Library { module, .. }
            | djls_project::TagProvenance::Builtin { module } => module.as_str(),
        };
        modules.insert(module.to_string());
    }

    let search_paths = build_search_paths(interpreter, root, pythonpath);

    let mut result = djls_extraction::ExtractionResult::default();

    for module_path in &modules {
        if let Some(file_path) = resolve_module_to_file(module_path, &search_paths) {
            match std::fs::read_to_string(file_path.as_std_path()) {
                Ok(source) => {
                    let module_result = djls_extraction::extract_rules(&source, module_path);
                    result.merge(module_result);
                }
                Err(e) => {
                    tracing::debug!("Failed to read module file {}: {}", file_path, e);
                }
            }
        } else {
            tracing::debug!("Could not resolve module path to file: {}", module_path);
        }
    }

    result
}

/// Build a list of directories to search when resolving Python module paths.
///
/// Includes:
/// - The project root (for workspace modules)
/// - Explicit PYTHONPATH entries
/// - Site-packages from the virtual environment (if available)
fn build_search_paths(
    interpreter: &Interpreter,
    root: &Utf8Path,
    pythonpath: &[String],
) -> Vec<Utf8PathBuf> {
    let mut paths = Vec::new();

    // Project root
    paths.push(root.to_path_buf());

    // Explicit PYTHONPATH entries
    for p in pythonpath {
        let path = Utf8PathBuf::from(p);
        if path.is_dir() {
            paths.push(path);
        }
    }

    // Site-packages from venv
    if let Some(site_packages) = find_site_packages(interpreter, root) {
        paths.push(site_packages);
    }

    paths
}

/// Find the site-packages directory for the given interpreter.
fn find_site_packages(interpreter: &Interpreter, root: &Utf8Path) -> Option<Utf8PathBuf> {
    let venv_path = match interpreter {
        Interpreter::VenvPath(path) => Some(Utf8PathBuf::from(path)),
        Interpreter::Auto => {
            // Check common venv directories
            for dir in &[".venv", "venv", "env", ".env"] {
                let candidate = root.join(dir);
                if candidate.is_dir() {
                    return find_site_packages_in_venv(&candidate);
                }
            }
            None
        }
        Interpreter::InterpreterPath(_) => None,
    };

    venv_path.as_deref().and_then(find_site_packages_in_venv)
}

/// Find site-packages within a specific venv directory.
fn find_site_packages_in_venv(venv: &Utf8Path) -> Option<Utf8PathBuf> {
    let lib_dir = venv.join("lib");
    if !lib_dir.is_dir() {
        return None;
    }

    // On Linux/macOS: lib/pythonX.Y/site-packages
    if let Ok(entries) = std::fs::read_dir(lib_dir.as_std_path()) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("python") {
                let site_packages =
                    Utf8PathBuf::from_path_buf(entry.path().join("site-packages")).ok()?;
                if site_packages.is_dir() {
                    return Some(site_packages);
                }
            }
        }
    }

    // On Windows: Lib/site-packages (capitalized)
    let lib_site = venv.join("Lib").join("site-packages");
    if lib_site.is_dir() {
        return Some(lib_site);
    }

    None
}

/// Resolve a dotted Python module path to a file system path.
///
/// Converts `django.template.defaulttags` to `django/template/defaulttags.py`
/// and searches the provided paths. Also checks for `__init__.py` (package
/// modules).
fn resolve_module_to_file(module_path: &str, search_paths: &[Utf8PathBuf]) -> Option<Utf8PathBuf> {
    let relative = module_path.replace('.', "/");

    for base in search_paths {
        // Try as a regular module: base/module/path.py
        let py_file = base.join(format!("{relative}.py"));
        if py_file.is_file() {
            return Some(py_file);
        }

        // Try as a package: base/module/path/__init__.py
        let init_file = base.join(&relative).join("__init__.py");
        if init_file.is_file() {
            return Some(init_file);
        }
    }

    None
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
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::Mutex;

    use djls_conf::Settings;
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
            None,
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
    fn inspector_inventory_change_invalidates_compute_tag_specs() {
        let (mut db, event_log) = test_db_with_project();

        // Prime the cache
        let _specs = db.tag_specs();
        event_log.take();

        // Update inspector_inventory on the project
        let project = db.project.lock().unwrap().unwrap();
        let new_inventory = Some(TemplateTags::new(
            vec![],
            vec![],
            HashMap::default(),
            vec![],
        ));
        project.set_inspector_inventory(&mut db).to(new_inventory);

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

        // "Update" diagnostics with an identical value — manual comparison
        // in update_project_from_settings prevents the setter call
        let project = db.project.lock().unwrap().unwrap();
        let current = project.diagnostics(&db).clone();

        // Simulate manual comparison: value is the same, so we don't call the setter
        assert_eq!(project.diagnostics(&db), &current);
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
        let mut extraction = djls_extraction::ExtractionResult::default();
        extraction.block_specs.insert(
            djls_extraction::SymbolKey::tag("test.module", "mytag"),
            djls_extraction::BlockTagSpec {
                end_tag: Some("endmytag".to_string()),
                intermediates: vec![],
                opaque: false,
            },
        );
        project
            .set_extracted_external_rules(&mut db)
            .to(Some(extraction));

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
        let _result1 = super::extract_module_rules(&db, file);
        let events = event_log.take();
        assert!(
            was_executed(&db, &events, "extract_module_rules"),
            "extract_module_rules should execute on first call"
        );

        // Second call — cached
        let _result2 = super::extract_module_rules(&db, file);
        let events = event_log.take();
        assert!(
            !was_executed(&db, &events, "extract_module_rules"),
            "extract_module_rules should NOT re-execute on second call (cached)"
        );
    }

    #[test]
    fn file_revision_change_with_same_source_backdates() {
        let (mut db, event_log) = test_db_with_project();

        // Create and extract from a file (file doesn't exist, source is empty)
        let file = djls_source::File::new(&db, "/test/project/tags.py".into(), 0);
        let _result = super::extract_module_rules(&db, file);
        event_log.take();

        // Bump the file revision — but the source is still empty (file not in FS)
        file.set_revision(&mut db).to(1);

        // Salsa's backdate optimization: file.source() returns the same empty text,
        // so extract_module_rules does NOT re-execute (correct behavior)
        let _result = super::extract_module_rules(&db, file);
        let events = event_log.take();
        assert!(
            !was_executed(&db, &events, "extract_module_rules"),
            "extract_module_rules should NOT re-execute when source content is unchanged (backdate)"
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
        let result = super::extract_module_rules(&db, file);

        // Should extract the filter
        let key = djls_extraction::SymbolKey::filter("", "my_filter");
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

        // Initially None
        assert!(
            project.extracted_external_rules(&db).is_none(),
            "extracted_external_rules should be None initially"
        );

        // Set some extraction results
        let mut extraction = djls_extraction::ExtractionResult::default();
        extraction.block_specs.insert(
            djls_extraction::SymbolKey::tag("test.module", "mytag"),
            djls_extraction::BlockTagSpec {
                end_tag: Some("endmytag".to_string()),
                intermediates: vec![],
                opaque: false,
            },
        );
        project
            .set_extracted_external_rules(&mut db)
            .to(Some(extraction.clone()));

        let stored = project.extracted_external_rules(&db);
        assert!(stored.is_some());
        assert_eq!(stored.as_ref().unwrap().block_specs.len(), 1);
    }

    #[test]
    fn extracted_rules_change_invalidates_compute_tag_specs() {
        let (mut db, event_log) = test_db_with_project();

        // Prime the cache
        let _specs = db.tag_specs();
        event_log.take();

        // Set extraction results on the project
        let project = db.project.lock().unwrap().unwrap();
        let mut extraction = djls_extraction::ExtractionResult::default();
        extraction.block_specs.insert(
            djls_extraction::SymbolKey::tag("test.module", "customblock"),
            djls_extraction::BlockTagSpec {
                end_tag: Some("endcustomblock".to_string()),
                intermediates: vec![],
                opaque: false,
            },
        );
        project
            .set_extracted_external_rules(&mut db)
            .to(Some(extraction));

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

    #[test]
    fn resolve_module_to_file_finds_py_file() {
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let base = camino::Utf8Path::from_path(dir.path()).unwrap();

        // Create django/template/defaulttags.py
        let module_dir = base.join("django").join("template");
        std::fs::create_dir_all(module_dir.as_std_path()).unwrap();
        std::fs::write(module_dir.join("defaulttags.py").as_std_path(), "").unwrap();

        let search_paths = vec![base.to_path_buf()];
        let result = super::resolve_module_to_file("django.template.defaulttags", &search_paths);
        assert!(result.is_some());
        assert!(result.unwrap().as_str().ends_with("defaulttags.py"));
    }

    #[test]
    fn resolve_module_to_file_finds_package_init() {
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let base = camino::Utf8Path::from_path(dir.path()).unwrap();

        // Create myapp/templatetags/__init__.py
        let pkg_dir = base.join("myapp").join("templatetags");
        std::fs::create_dir_all(pkg_dir.as_std_path()).unwrap();
        std::fs::write(pkg_dir.join("__init__.py").as_std_path(), "").unwrap();

        let search_paths = vec![base.to_path_buf()];
        let result = super::resolve_module_to_file("myapp.templatetags", &search_paths);
        assert!(result.is_some());
        assert!(result.unwrap().as_str().ends_with("__init__.py"));
    }

    #[test]
    fn resolve_module_to_file_returns_none_for_missing() {
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let base = camino::Utf8Path::from_path(dir.path()).unwrap();
        let search_paths = vec![base.to_path_buf()];
        let result = super::resolve_module_to_file("nonexistent.module", &search_paths);
        assert!(result.is_none());
    }
}

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
use djls_extraction::extract_rules;
use djls_extraction::ExtractionResult;
use djls_project::query;
use djls_project::resolve_modules;
use djls_project::template_dirs;
use djls_project::Db as ProjectDb;
use djls_project::Inspector;
use djls_project::InspectorInventory;
use djls_project::Interpreter;

use djls_project::Project;
use djls_project::PythonEnvRequest;
use djls_project::TemplateInventoryRequest;
use djls_semantic::TagSpec;
use djls_semantic::Db as SemanticDb;
use djls_semantic::TagIndex;
use djls_semantic::TagSpecs;
use djls_semantic::django_builtin_specs;
use djls_source::Db as SourceDb;
use djls_source::File;
use djls_source::FileKind;
use djls_source::FxDashMap;
use djls_templates::Db as TemplateDb;
use djls_workspace::Db as WorkspaceDb;
use djls_workspace::FileSystem;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;
use salsa::Setter;

/// Extract validation rules from a workspace Python module.
///
/// This is a TRACKED QUERY: changes to the File input (revision bump)
/// automatically invalidate cached results.
///
/// Only extracts from Python files. Returns empty result for other file types.
#[salsa::tracked]
pub fn extract_workspace_module_rules(db: &dyn SemanticDb, file: File) -> ExtractionResult {
    // Check file type
    let source = file.source(db);
    if source.kind() != &FileKind::Python {
        return ExtractionResult::default();
    }

    // Extract rules using djls-extraction
    match extract_rules(source.as_str()) {
        Ok(result) => result,
        Err(e) => {
            tracing::warn!("Extraction failed for {}: {}", file.path(db), e);
            ExtractionResult::default()
        }
    }
}

/// Collect extracted rules from all workspace registration modules.
///
/// This tracked query:
/// 1. Gets registration modules from inspector inventory
/// 2. Resolves workspace modules to File inputs
/// 3. Extracts rules from each (via tracked `extract_workspace_module_rules`)
///
/// External modules are handled separately (cached on Project).
#[salsa::tracked]
pub fn collect_workspace_extraction_results(
    db: &dyn SemanticDb,
    project: Project,
) -> Vec<(String, ExtractionResult)> {
    let inventory = project.inspector_inventory(db);
    let sys_path = project.sys_path(db);
    let root = project.root(db);

    let Some(inventory) = inventory else {
        return Vec::new();
    };

    // Get unique registration module paths from inventory (tags + filters).
    //
    // IMPORTANT: some registration modules may define only filters (no tags). If we only
    // enumerate from tags, we will miss those modules and never extract filter arity/specs.
    let mut module_paths = FxHashSet::<String>::default();
    for tag in inventory.tags() {
        module_paths.insert(tag.registration_module().to_string());
    }
    for filter in inventory.filters() {
        module_paths.insert(filter.registration_module().to_string());
    }

    // Resolve and partition by location
    let (workspace_modules, _external) =
        resolve_modules(module_paths.iter().map(String::as_str), sys_path, root);

    let mut results = Vec::new();

    for resolved in workspace_modules {
        // Get or create File for this workspace module
        let file = db
            .get_file(&resolved.file_path)
            .unwrap_or_else(|| db.create_file(&resolved.file_path));

        // Extract via tracked query (establishes Salsa dependency)
        let extraction = extract_workspace_module_rules(db, file);

        if !extraction.is_empty() {
            results.push((resolved.module_path, extraction));
        }
    }

    results
}

/// Compute tag specifications from all sources.
///
/// This tracked query merges:
/// 1. Django builtin specs (compile-time constant)
/// 2. Extracted rules from workspace modules (tracked queries)
/// 3. Extracted rules from external modules (Project field)
/// 4. User config overrides (Project.tagspecs field)
///
/// Invalidation triggers:
/// - Workspace Python file changes → via `extract_workspace_module_rules` dependency
/// - External modules change → via `Project.extracted_external_rules`
/// - User config changes → via `Project.tagspecs`
#[salsa::tracked]
pub fn compute_tag_specs(db: &dyn SemanticDb, project: Project) -> TagSpecs {
    // Start with Django builtins (compile-time constant)
    let mut specs = django_builtin_specs();

    // Merge workspace extraction results (tracked)
    let workspace_results = collect_workspace_extraction_results(db, project);
    for (module_path, extraction) in workspace_results {
        merge_extraction_into_specs(&mut specs, &module_path, &extraction);
    }

    // Merge external extraction results (from Project field)
    let external_results = project.extracted_external_rules(db);
    for (module_path, extraction) in external_results {
        merge_extraction_into_specs(&mut specs, module_path, extraction);
    }

    // Apply user config overrides (highest priority)
    let user_specs = TagSpecs::from_config_def(project.tagspecs(db));
    specs.merge(user_specs);

    tracing::trace!("Computed tag specs: {} tags total", specs.len());

    specs
}

/// Merge extraction results into tag specs.
fn merge_extraction_into_specs(
    specs: &mut TagSpecs,
    module_path: &str,
    extraction: &ExtractionResult,
) {
    for tag in &extraction.tags {
        // Look up existing spec or create new one
        if let Some(spec) = specs.get_mut(&tag.name) {
            // Enrich existing spec with extracted rules
            spec.merge_extracted_rules(&tag.rules);
            if let Some(ref block_spec) = tag.block_spec {
                spec.merge_block_spec(block_spec);
            }
            spec.populate_args_from_extraction(&tag.extracted_args);
        } else {
            // Create new spec from extraction
            let mut new_spec = TagSpec::from_extraction(module_path, tag);
            new_spec.populate_args_from_extraction(&tag.extracted_args);
            specs.insert(tag.name.clone(), new_spec);
        }
    }
}

/// Build the tag index from computed tag specs.
#[salsa::tracked]
pub fn compute_tag_index(db: &dyn SemanticDb, project: Project) -> TagIndex<'_> {
    let specs = compute_tag_specs(db, project);
    TagIndex::from_specs(db, &specs)
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
mod test_infrastructure {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Test logger that stores raw Salsa events for stable identification.
    #[derive(Clone, Default)]
    pub struct EventLogger {
        events: Arc<Mutex<Vec<salsa::Event>>>,
    }

    impl EventLogger {
        pub fn push(&self, event: salsa::Event) {
            self.events.lock().unwrap().push(event);
        }

        pub fn take(&self) -> Vec<salsa::Event> {
            std::mem::take(&mut *self.events.lock().unwrap())
        }

        pub fn clear(&self) {
            self.events.lock().unwrap().clear();
        }

        /// Check if a query was executed by looking for a matching ingredient debug name
        /// in `WillExecute` events.
        pub fn was_executed(&self, db: &dyn salsa::Database, query_name: &str) -> bool {
            self.events.lock().unwrap().iter().any(|event| match event.kind {
                salsa::EventKind::WillExecute { database_key } => {
                    db.ingredient_debug_name(database_key.ingredient_index()) == query_name
                }
                _ => false,
            })
        }
    }

    /// Test database with event logging (Salsa pattern)
    pub struct TestDatabase {
        pub db: DjangoDatabase,
        pub logger: EventLogger,
    }

    impl TestDatabase {
        pub fn new() -> Self {
            let logger = EventLogger::default();
            let db = Self::create_db_with_logger(&logger);
            Self { db, logger }
        }

        pub fn with_project() -> Self {
            let test_db = Self::new();
            let settings = Settings::default();

            // Create project directly (bypass bootstrap which needs real files)
            let project = Project::new(
                &test_db.db,
                Utf8PathBuf::from("/test/project"),
                Interpreter::discover(None),
                Some("test.settings".to_string()),
                vec![],
                None,                           // inspector_inventory
                settings.tagspecs().clone(),    // tagspecs
                settings.diagnostics().clone(), // diagnostics
                Vec::new(),                     // sys_path
                FxHashMap::default(),           // extracted_external_rules
            );
            *test_db.db.project.lock().unwrap() = Some(project);

            test_db
        }

        fn create_db_with_logger(logger: &EventLogger) -> DjangoDatabase {
            use djls_workspace::InMemoryFileSystem;

            DjangoDatabase {
                fs: Arc::new(InMemoryFileSystem::new()),
                files: Arc::new(FxDashMap::default()),
                project: Arc::new(Mutex::new(None)),
                settings: Arc::new(Mutex::new(Settings::default())),
                inspector: Arc::new(Inspector::new()),
                storage: salsa::Storage::new(Some(Box::new({
                    let logger = logger.clone();
                    move |event| {
                        logger.push(event);
                    }
                }))),
                #[cfg(test)]
                logs: Arc::new(Mutex::new(None)),
            }
        }
    }
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

    #[cfg(test)]
    fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    /// Update the settings and propagate changes to the Project.
    ///
    /// This method delegates to `update_project_from_settings()` to ensure
    /// manual comparison is performed before calling Salsa setters.
    ///
    /// # Panics
    ///
    /// Panics if the settings mutex is poisoned (another thread panicked while holding the lock)
    pub fn set_settings(&mut self, settings: &Settings) {
        // Store new settings in mutex (still needed for non-Project uses)
        *self.settings.lock().unwrap() = settings.clone();

        if let Some(project) = self.project() {
            // Update Project fields with comparison
            self.update_project_from_settings(project, settings);
        }
    }

    /// Initialize or update the project.
    ///
    /// If no project exists, creates one. If project exists, updates
    /// fields via setters ONLY when values actually changed (Ruff/RA style).
    fn set_project(&mut self, root: &Utf8Path, settings: &Settings) {
        if self.project().is_some() {
            // Project exists - update via setters with manual comparison
            if let Some(project) = self.project() {
                self.update_project_from_settings(project, settings);
            }
        } else {
            // No project yet - create one
            let project = Project::bootstrap(
                self,
                root,
                settings.venv_path(),
                settings.django_settings_module(),
                settings.pythonpath(),
                settings,
            );
            *self.project.lock().unwrap() = Some(project);

            // Refresh inspector after project creation
            self.refresh_inspector();
        }
    }

    /// Update Project fields from settings, comparing before setting.
    ///
    /// Only calls Salsa setters when values actually changed.
    /// This is the Ruff/RA pattern to avoid unnecessary invalidation.
    fn update_project_from_settings(&mut self, project: Project, settings: &Settings) {
        let mut env_changed = false;

        // Check and update interpreter
        let new_interpreter = Interpreter::discover(settings.venv_path());
        if project.interpreter(self) != &new_interpreter {
            project.set_interpreter(self).to(new_interpreter);
            env_changed = true;
        }

        // Check and update django_settings_module
        let new_dsm = settings.django_settings_module().map(String::from);
        if project.django_settings_module(self) != &new_dsm {
            project.set_django_settings_module(self).to(new_dsm);
            env_changed = true;
        }

        // Check and update pythonpath
        let new_pp = settings.pythonpath().to_vec();
        if project.pythonpath(self) != &new_pp {
            project.set_pythonpath(self).to(new_pp);
            env_changed = true;
        }

        // Check and update tagspecs (config doc, not TagSpecs!)
        let new_tagspecs = settings.tagspecs().clone();
        if project.tagspecs(self) != &new_tagspecs {
            tracing::debug!("Tagspecs config changed, updating Project");
            project.set_tagspecs(self).to(new_tagspecs);
        }

        // Check and update diagnostics
        let new_diagnostics = settings.diagnostics().clone();
        if project.diagnostics(self) != &new_diagnostics {
            tracing::debug!("Diagnostics config changed, updating Project");
            project.set_diagnostics(self).to(new_diagnostics);
        }

        // Refresh inspector if environment changed
        if env_changed {
            tracing::debug!("Python environment changed, refreshing inspector");
            self.refresh_inspector();
        }
    }

    /// Refresh the inspector inventory AND extract rules from external modules.
    ///
    /// This method:
    /// 1. Queries Python for `sys_path` (`python_env`)
    /// 2. Queries Python for inventory (tags + filters)
    /// 3. Resolves external registration modules
    /// 4. Extracts rules from external modules (reads file content via fs)
    /// 5. Updates Project fields with manual comparison
    ///
    /// Called when:
    /// - Project is first initialized
    /// - Python environment changes (venv, PYTHONPATH)
    /// - User explicitly requests refresh (e.g., after pip install)
    pub fn refresh_inspector(&mut self) {
        let Some(project) = self.project() else {
            tracing::warn!("Cannot refresh inspector: no project set");
            return;
        };

        // 1. Refresh sys_path from python_env query
        let new_sys_path = self.query_sys_path();
        if project.sys_path(self) != &new_sys_path {
            project.set_sys_path(self).to(new_sys_path.clone());
        }

        // 2. Refresh inventory (existing logic)
        let new_inventory = self.query_inventory();
        let inventory_changed = project.inspector_inventory(self) != &new_inventory;
        if inventory_changed {
            project.set_inspector_inventory(self).to(new_inventory.clone());
        }

        // 3. Extract from external modules (only if inventory changed or first run)
        if inventory_changed || project.extracted_external_rules(self).is_empty() {
            let new_external_rules =
                self.extract_external_module_rules(new_inventory.as_ref(), &new_sys_path, project.root(self));

            if project.extracted_external_rules(self) != &new_external_rules {
                tracing::debug!("External extraction updated: {} modules", new_external_rules.len());
                project
                    .set_extracted_external_rules(self)
                    .to(new_external_rules);
            }
        }
    }

    /// Query `sys_path` from Python environment.
    fn query_sys_path(&self) -> Vec<Utf8PathBuf> {
        if let Some(response) = query(self, &PythonEnvRequest) {
            response.sys_path.into_iter().collect()
        } else {
            tracing::warn!("Failed to query sys_path");
            Vec::new()
        }
    }

    /// Query inventory from inspector.
    fn query_inventory(&self) -> Option<InspectorInventory> {
        query(self, &TemplateInventoryRequest).map(InspectorInventory::from_response)
    }

    /// Extract rules from external modules.
    ///
    /// Reads file content directly from filesystem (not via Salsa File inputs).
    /// These are external to the project and don't need automatic invalidation.
    fn extract_external_module_rules(
        &self,
        inventory: Option<&InspectorInventory>,
        sys_path: &[Utf8PathBuf],
        project_root: &Utf8Path,
    ) -> FxHashMap<String, ExtractionResult> {
        let Some(inventory) = inventory else {
            return FxHashMap::default();
        };

        // Enumerate registration modules from inventory (tags + filters).
        // This must include filter-only modules, otherwise we will miss extracted filter arity/specs.
        let mut module_paths = FxHashSet::<String>::default();
        for tag in inventory.tags() {
            module_paths.insert(tag.registration_module().to_string());
        }
        for filter in inventory.filters() {
            module_paths.insert(filter.registration_module().to_string());
        }

        let (_workspace, external_modules) =
            resolve_modules(module_paths.iter().map(String::as_str), sys_path, project_root);

        let mut results = FxHashMap::default();

        for resolved in external_modules {
            // Read file content directly (NOT via Salsa)
            let Ok(source) = self.fs.read_to_string(&resolved.file_path) else {
                continue;
            };

            match extract_rules(&source) {
                Ok(extraction) if !extraction.is_empty() => {
                    results.insert(resolved.module_path, extraction);
                }
                Ok(_) => {} // Empty extraction, skip
                Err(e) => {
                    tracing::debug!(
                        "Extraction failed for external module {}: {}",
                        resolved.module_path,
                        e
                    );
                }
            }
        }

        results
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
            // Read inputs to establish Salsa dependencies
            let _inventory = project.inspector_inventory(self);
            let _external_rules = project.extracted_external_rules(self);
            let _tagspecs = project.tagspecs(self);
            compute_tag_specs(self, project)
        } else {
            django_builtin_specs()
        }
    }

    fn tag_index(&self) -> TagIndex<'_> {
        if let Some(project) = self.project() {
            compute_tag_index(self, project)
        } else {
            let specs = django_builtin_specs();
            TagIndex::from_specs(self, &specs)
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

    fn inspector_inventory(&self) -> Option<&InspectorInventory> {
        self.project()
            .and_then(|project| project.inspector_inventory(self).as_ref())
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
    use super::*;
    use super::test_infrastructure::*;
    use std::collections::HashMap;

    #[test]
    fn test_tag_specs_cached_on_repeated_access() {
        let test = TestDatabase::with_project();

        // First access - should execute query
        let _specs1 = test.db.tag_specs();
        assert!(
            test.logger.was_executed(&test.db, "compute_tag_specs"),
            "compute_tag_specs should execute on first access.\nLogs: {:?}",
            test.logger.take()
        );

        test.logger.clear();

        // Second access - should use cache (no WillExecute event)
        let _specs2 = test.db.tag_specs();
        assert!(
            !test.logger.was_executed(&test.db, "compute_tag_specs"),
            "compute_tag_specs should be cached on second access"
        );
    }

    #[test]
    fn test_tagspecs_change_invalidates() {
        let mut test = TestDatabase::with_project();

        // First access
        let _specs1 = test.db.tag_specs();
        test.logger.clear();

        // Update tagspecs via Project setter
        let project = test.db.project().expect("test project exists");
        let mut new_tagspecs = project.tagspecs(&test.db).clone();
        // Modify tagspecs (add a library)
        new_tagspecs.libraries.push(djls_conf::TagLibraryDef {
            module: "test.templatetags".to_string(),
            requires_engine: None,
            tags: vec![],
            extra: None,
        });

        // Manual comparison shows change - set it
        assert!(project.tagspecs(&test.db) != &new_tagspecs);
        project.set_tagspecs(&mut test.db).to(new_tagspecs);

        // Access again - should recompute
        let _specs2 = test.db.tag_specs();
        assert!(
            test.logger.was_executed(&test.db, "compute_tag_specs"),
            "compute_tag_specs should recompute after tagspecs change"
        );
    }

    #[test]
    fn test_inspector_inventory_change_invalidates() {
        use djls_project::InspectorInventory;
        use djls_project::TemplateTag;

        let mut test = TestDatabase::with_project();

        // First access
        let _specs1 = test.db.tag_specs();
        test.logger.clear();

        // Update inspector inventory with a tag
        let project = test.db.project().expect("test project exists");
        let new_inventory = InspectorInventory::new(
            HashMap::new(),
            vec!["django.template.defaulttags".to_string()],
            vec![TemplateTag::new_builtin(
                "if",
                "django.template.defaulttags",
                None,
            )],
            vec![], // no filters
        );
        project.set_inspector_inventory(&mut test.db).to(Some(new_inventory));

        // Access again - should recompute workspace extraction (which depends on inventory)
        let _specs2 = test.db.tag_specs();

        // The dependency chain is: compute_tag_specs -> collect_workspace_extraction_results -> inspector_inventory
        // When inventory changes, collect_workspace_extraction_results should re-execute
        assert!(
            test.logger.was_executed(&test.db, "collect_workspace_extraction_results"),
            "collect_workspace_extraction_results should recompute after inventory change"
        );

        // Note: compute_tag_specs may not re-execute if its output would be identical
        // (Salsa's incremental computation avoids unnecessary work)
    }

    #[test]
    fn test_same_value_no_invalidation() {
        let test = TestDatabase::with_project();

        // First access
        let _specs1 = test.db.tag_specs();
        test.logger.clear();

        // "Update" with same value - should NOT call setter
        let project = test.db.project().expect("test project exists");
        let same_tagspecs = project.tagspecs(&test.db).clone();

        // Manual comparison shows NO change - don't set
        assert!(project.tagspecs(&test.db) == &same_tagspecs);
        // Note: We don't call set_tagspecs because values are equal

        // Access again - should NOT recompute
        let _specs2 = test.db.tag_specs();
        assert!(
            !test.logger.was_executed(&test.db, "compute_tag_specs"),
            "compute_tag_specs should NOT recompute when value unchanged"
        );
    }

    #[test]
    fn test_tag_index_depends_on_tag_specs() {
        let mut test = TestDatabase::with_project();

        // Access tag_index (triggers tag_specs)
        let _index1 = test.db.tag_index();
        assert!(
            test.logger.was_executed(&test.db, "compute_tag_specs"),
            "tag_index should trigger tag_specs"
        );

        test.logger.clear();

        // Change tagspecs
        let project = test.db.project().expect("test project exists");
        let mut new_tagspecs = project.tagspecs(&test.db).clone();
        new_tagspecs.libraries.push(djls_conf::TagLibraryDef {
            module: "another.templatetags".to_string(),
            requires_engine: None,
            tags: vec![],
            extra: None,
        });
        project.set_tagspecs(&mut test.db).to(new_tagspecs);

        // Access tag_index - should recompute
        let _index2 = test.db.tag_index();
        assert!(
            test.logger.was_executed(&test.db, "compute_tag_specs"),
            "tag_index should recompute when tagspecs change"
        );
    }

    #[test]
    fn test_update_project_from_settings_compares() {
        let mut test = TestDatabase::with_project();

        // First access
        let _specs1 = test.db.tag_specs();
        test.logger.clear();

        // Call update_project_from_settings with same settings
        let project = test.db.project().expect("test project exists");
        let settings = test.db.settings();
        test.db.update_project_from_settings(project, &settings);

        // Should NOT invalidate (manual comparison prevents setter calls)
        let _specs2 = test.db.tag_specs();
        assert!(
            !test.logger.was_executed(&test.db, "compute_tag_specs"),
            "update_project_from_settings should not invalidate when unchanged"
        );
    }
}

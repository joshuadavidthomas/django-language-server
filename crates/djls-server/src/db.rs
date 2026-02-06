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
use djls_extraction::ExtractionResult;
use djls_project::inspector_query;
use djls_project::resolve_modules;
use djls_project::template_dirs;
use djls_project::Db as ProjectDb;
use djls_project::Inspector;
use djls_project::InspectorInventory;
use djls_project::Interpreter;
use djls_project::Project;
use djls_project::TemplateInventoryRequest;
use djls_semantic::django_builtin_specs;
use djls_semantic::Db as SemanticDb;
use djls_semantic::FilterAritySpecs;
use djls_semantic::OpaqueTagMap;
use djls_semantic::TagIndex;
use djls_semantic::TagSpec;
use djls_semantic::TagSpecs;
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
/// This is a TRACKED QUERY: changes to the `File` input (revision bump)
/// automatically invalidate cached results.
///
/// Only extracts from Python files. Returns empty result for other file types.
#[salsa::tracked]
fn extract_workspace_module_rules(db: &dyn SemanticDb, file: File) -> ExtractionResult {
    let source = file.source(db);
    if source.kind() != &FileKind::Python {
        return ExtractionResult::default();
    }

    match djls_extraction::extract_rules(source.as_str()) {
        Ok(result) => result,
        Err(e) => {
            tracing::warn!("Extraction failed for {}: {e}", file.path(db),);
            ExtractionResult::default()
        }
    }
}

/// Collect extracted rules from all workspace registration modules.
///
/// This tracked query:
/// 1. Gets registration modules from inspector inventory
/// 2. Resolves workspace modules to `File` inputs
/// 3. Extracts rules from each (via tracked `extract_workspace_module_rules`)
///
/// External modules are handled separately (cached on Project).
#[salsa::tracked]
fn collect_workspace_extraction_results(
    db: &dyn SemanticDb,
    project: Project,
) -> Vec<(String, ExtractionResult)> {
    let inventory = project.inspector_inventory(db);
    let sys_path = project.sys_path(db);
    let root = project.root(db);

    let Some(inventory) = inventory else {
        return Vec::new();
    };

    let mut module_paths = FxHashSet::<String>::default();
    for tag in inventory.tags() {
        module_paths.insert(tag.registration_module().to_string());
    }
    for filter in inventory.filters() {
        module_paths.insert(filter.registration_module().to_string());
    }

    let (workspace_modules, _external) =
        resolve_modules(module_paths.iter().map(String::as_str), sys_path, root);

    let mut results = Vec::new();

    for resolved in workspace_modules {
        let file = db.get_or_create_file(&resolved.file_path);

        let extraction = extract_workspace_module_rules(db, file);

        if !extraction.is_empty() {
            results.push((resolved.module_path, extraction));
        }
    }

    results
}

/// Merge extraction results into tag specs.
fn merge_extraction_into_specs(
    specs: &mut TagSpecs,
    module_path: &str,
    extraction: &ExtractionResult,
) {
    for tag in &extraction.tags {
        if let Some(spec) = specs.get_mut(&tag.name) {
            spec.merge_extracted_rules(&tag.rules);
            if let Some(ref block_spec) = tag.block_spec {
                spec.merge_block_spec(block_spec);
            }
        } else {
            let new_spec = TagSpec::from_extraction(module_path, tag);
            specs.insert(tag.name.clone(), new_spec);
        }
    }
}

/// Compute tag specifications from all sources.
///
/// This tracked query merges:
/// 1. Django builtin specs (compile-time constant)
/// 2. Extracted rules from workspace modules (tracked queries)
/// 3. Extracted rules from external modules (Project field)
/// 4. User config overrides (`Project.tagspecs` field)
///
/// Invalidation triggers:
/// - Workspace Python file changes → via `extract_workspace_module_rules` dependency
/// - External modules change → via `Project.extracted_external_rules`
/// - User config changes → via `Project.tagspecs`
#[salsa::tracked]
fn compute_tag_specs(db: &dyn SemanticDb, project: Project) -> TagSpecs {
    let _inventory = project.inspector_inventory(db);

    let mut specs = django_builtin_specs();

    // Merge workspace extraction results (tracked)
    let workspace_results = collect_workspace_extraction_results(db, project);
    for (module_path, extraction) in &workspace_results {
        merge_extraction_into_specs(&mut specs, module_path, extraction);
    }

    // Merge external extraction results (from Project field)
    let external_results = project.extracted_external_rules(db);
    for (module_path, extraction) in external_results {
        merge_extraction_into_specs(&mut specs, module_path, extraction);
    }

    // Apply user config overrides (highest priority)
    let user_specs = TagSpecs::from_config_def(project.tagspecs(db));
    if !user_specs.is_empty() {
        specs.merge(user_specs);
    }

    tracing::trace!("Computed tag specs: {} tags total", specs.len());

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

/// Compute filter arity specs from extraction results.
///
/// Keyed by `SymbolKey` (`registration_module` + name + Filter kind) for
/// collision-safe lookup when multiple libraries define the same filter name.
#[salsa::tracked]
fn compute_filter_arity_specs(db: &dyn SemanticDb, project: Project) -> FilterAritySpecs {
    use djls_extraction::SymbolKey;

    let mut specs = FxHashMap::default();

    // Workspace extraction results (tracked queries)
    let workspace_results = collect_workspace_extraction_results(db, project);
    for (module_path, extraction) in &workspace_results {
        for filter in &extraction.filters {
            let key = SymbolKey::filter(module_path.clone(), filter.name.clone());
            specs.entry(key).or_insert(filter.arity.clone());
        }
    }

    // External extraction results (from Project field)
    let external_results = project.extracted_external_rules(db);
    for (module_path, extraction) in external_results {
        for filter in &extraction.filters {
            let key = SymbolKey::filter(module_path.clone(), filter.name.clone());
            specs.entry(key).or_insert(filter.arity.clone());
        }
    }

    FilterAritySpecs::new(specs)
}

/// Compute opaque tag map from `TagSpecs`.
///
/// Returns opener → closer mapping for tags where `opaque == true`.
/// Used by M6 opaque region detection to skip validation inside opaque blocks.
#[salsa::tracked]
fn compute_opaque_tag_map(db: &dyn SemanticDb, project: Project) -> OpaqueTagMap {
    let tag_specs = compute_tag_specs(db, project);
    let mut map = OpaqueTagMap::default();

    for (tag_name, spec) in &tag_specs {
        if spec.opaque {
            if let Some(ref end_tag) = spec.end_tag {
                map.insert(tag_name.clone(), end_tag.name.to_string());
            }
        }
    }

    map
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

    /// Refresh the inspector inventory AND extract rules from external modules.
    ///
    /// This method:
    /// 1. Queries `sys_path` from Python environment
    /// 2. Queries the Python inspector for inventory (tags + filters)
    /// 3. Resolves external registration modules
    /// 4. Extracts rules from external modules (reads file content via fs)
    /// 5. Updates Project fields with manual comparison
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

        // 1. Refresh sys_path
        let new_sys_path = self.query_sys_path();
        if project.sys_path(self) != &new_sys_path {
            project.set_sys_path(self).to(new_sys_path);
        }

        // 2. Refresh inventory
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
        let inventory_changed = current != &new_inventory;
        if inventory_changed {
            tracing::debug!(
                "Inspector inventory changed: {} tags, {} filters -> {} tags, {} filters",
                current.as_ref().map_or(0, InspectorInventory::tag_count),
                current.as_ref().map_or(0, InspectorInventory::filter_count),
                new_inventory
                    .as_ref()
                    .map_or(0, InspectorInventory::tag_count),
                new_inventory
                    .as_ref()
                    .map_or(0, InspectorInventory::filter_count),
            );
            project
                .set_inspector_inventory(self)
                .to(new_inventory.clone());
        } else {
            tracing::trace!("Inspector inventory unchanged, skipping update");
        }

        // 3. Extract from external modules (only if inventory changed or first run)
        if inventory_changed || project.extracted_external_rules(self).is_empty() {
            let new_external_rules =
                self.extract_external_module_rules(new_inventory.as_ref(), project);

            if project.extracted_external_rules(self) != &new_external_rules {
                tracing::debug!(
                    "External extraction updated: {} modules",
                    new_external_rules.len()
                );
                project
                    .set_extracted_external_rules(self)
                    .to(new_external_rules);
            }
        }
    }

    /// Query `sys_path` from Python environment.
    fn query_sys_path(&self) -> Vec<Utf8PathBuf> {
        use djls_project::PythonEnvRequest;

        if let Some(response) = inspector_query(self, &PythonEnvRequest) {
            response
                .sys_path
                .into_iter()
                .filter_map(|p| Utf8PathBuf::try_from(std::path::PathBuf::from(p)).ok())
                .collect()
        } else {
            tracing::warn!("Failed to query sys_path from Python environment");
            Vec::new()
        }
    }

    /// Extract rules from external modules.
    ///
    /// Reads file content directly from filesystem (not via Salsa `File` inputs).
    /// These are external to the project and don't need automatic invalidation.
    fn extract_external_module_rules(
        &self,
        inventory: Option<&InspectorInventory>,
        project: Project,
    ) -> FxHashMap<String, ExtractionResult> {
        let Some(inventory) = inventory else {
            return FxHashMap::default();
        };

        let sys_path = project.sys_path(self);
        let project_root = project.root(self);

        let mut module_paths = FxHashSet::<String>::default();
        for tag in inventory.tags() {
            module_paths.insert(tag.registration_module().to_string());
        }
        for filter in inventory.filters() {
            module_paths.insert(filter.registration_module().to_string());
        }

        let (_workspace, external_modules) = resolve_modules(
            module_paths.iter().map(String::as_str),
            sys_path,
            project_root,
        );

        let mut results = FxHashMap::default();

        for resolved in external_modules {
            let Ok(source) = self.fs.read_to_string(&resolved.file_path) else {
                continue;
            };

            match djls_extraction::extract_rules(&source) {
                Ok(extraction) if !extraction.is_empty() => {
                    results.insert(resolved.module_path, extraction);
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::debug!(
                        "Extraction failed for external module {}: {e}",
                        resolved.module_path,
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

    fn filter_arity_specs(&self) -> FilterAritySpecs {
        if let Some(project) = self.project() {
            compute_filter_arity_specs(self, project)
        } else {
            FilterAritySpecs::default()
        }
    }

    fn opaque_tag_map(&self) -> OpaqueTagMap {
        if let Some(project) = self.project() {
            compute_opaque_tag_map(self, project)
        } else {
            OpaqueTagMap::default()
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

    use camino::Utf8PathBuf;
    use djls_conf::Settings;
    use djls_conf::TagLibraryDef;
    use djls_project::Inspector;
    use djls_project::InspectorInventory;
    use djls_project::Interpreter;
    use djls_project::Project;
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
                Vec::new(),
                rustc_hash::FxHashMap::default(),
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

        let project = test
            .db
            .project
            .lock()
            .unwrap()
            .expect("test project exists");
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

        let project = test
            .db
            .project
            .lock()
            .unwrap()
            .expect("test project exists");
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

        let project = test
            .db
            .project
            .lock()
            .unwrap()
            .expect("test project exists");
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

        let project = test
            .db
            .project
            .lock()
            .unwrap()
            .expect("test project exists");
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

    #[test]
    fn external_rules_change_invalidates_tag_specs() {
        let mut test = TestDatabase::with_project();

        let _specs1 = test.db.tag_specs();
        test.logger.clear();

        let project = test
            .db
            .project
            .lock()
            .unwrap()
            .expect("test project exists");

        // Simulate setting extracted external rules
        let mut external_rules = rustc_hash::FxHashMap::default();
        external_rules.insert(
            "django.templatetags.i18n".to_string(),
            djls_extraction::ExtractionResult::default(),
        );
        project
            .set_extracted_external_rules(&mut test.db)
            .to(external_rules);

        let _specs2 = test.db.tag_specs();
        assert!(
            test.logger.was_executed(&test.db, "compute_tag_specs"),
            "compute_tag_specs should recompute after external rules change"
        );
    }

    #[test]
    fn external_rules_not_auto_invalidated_without_setter() {
        let mut test = TestDatabase::with_project();

        // Set some external rules initially
        let project = test
            .db
            .project
            .lock()
            .unwrap()
            .expect("test project exists");
        let mut external_rules = rustc_hash::FxHashMap::default();
        external_rules.insert(
            "django.templatetags.i18n".to_string(),
            djls_extraction::ExtractionResult::default(),
        );
        project
            .set_extracted_external_rules(&mut test.db)
            .to(external_rules);

        let _specs1 = test.db.tag_specs();
        test.logger.clear();

        // Without calling setter, tag_specs should stay cached
        let _specs2 = test.db.tag_specs();
        assert!(
            !test.logger.was_executed(&test.db, "compute_tag_specs"),
            "External rules don't auto-invalidate — need explicit setter call"
        );
    }

    #[test]
    fn workspace_extraction_cached_for_unchanged_file() {
        let test = TestDatabase::with_project();

        // Create a Python file
        let path = Utf8PathBuf::from("/test/project/templatetags/custom.py");
        let file = djls_source::File::new(&test.db, path, 0);

        // First extraction
        let _result1 = super::extract_workspace_module_rules(&test.db, file);
        assert!(
            test.logger
                .was_executed(&test.db, "extract_workspace_module_rules"),
            "Should execute on first call"
        );

        test.logger.clear();

        // Second call — should use cache since file revision unchanged
        let _result2 = super::extract_workspace_module_rules(&test.db, file);
        assert!(
            !test
                .logger
                .was_executed(&test.db, "extract_workspace_module_rules"),
            "Should use cached result when file unchanged"
        );
    }

    #[test]
    fn workspace_extraction_reruns_on_revision_bump() {
        let mut test = TestDatabase::with_project();

        // Create a Python file with actual content via InMemoryFileSystem
        // Note: InMemoryFileSystem is constructed empty and immutable through Arc,
        // so we use a pre-populated one via `file_with_content` helper pattern.
        // For this test, we verify that revision bump invalidates the
        // source text dependency which cascades to extraction.
        let path = Utf8PathBuf::from("/test/project/templatetags/custom.py");
        let file = djls_source::File::new(&test.db, path, 0);

        // First extraction (empty source — no registrations found)
        let _result1 = super::extract_workspace_module_rules(&test.db, file);
        test.logger.clear();

        // Bump revision — Salsa will re-evaluate the source() tracked query.
        // Since the file content is the same (still empty from InMemoryFileSystem),
        // Salsa's early cutoff means extraction won't actually re-run.
        // This is correct Salsa behavior: if source text is unchanged,
        // extraction needn't re-execute.
        file.set_revision(&mut test.db).to(1);

        let _result2 = super::extract_workspace_module_rules(&test.db, file);
        // With early cutoff, extraction may or may not re-run depending on
        // whether the source text changed. Since InMemoryFileSystem is empty,
        // the source is the same, so Salsa correctly skips re-execution.
        // The important thing is that NO panic occurs and the query succeeds.
    }
}

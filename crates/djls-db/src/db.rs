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
use djls_project::Project;
use djls_project::TemplateLibraries;
use djls_semantic::Db as SemanticDb;
use djls_semantic::TagIndex;
use djls_semantic::TagSpecs;
use djls_source::Db as SourceDb;
use djls_source::File;
use djls_source::FxDashMap;
use djls_templates::Db as TemplateDb;
use djls_workspace::Db as WorkspaceDb;
use djls_workspace::FileSystem;

use crate::queries::compute_filter_arity_specs;
use crate::queries::compute_tag_index;
use crate::queries::compute_tag_specs;

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
    pub(crate) fs: Arc<dyn FileSystem>,

    /// Registry of tracked files used by the workspace layer.
    pub(crate) files: Arc<FxDashMap<Utf8PathBuf, File>>,

    /// The single project for this database instance
    pub(crate) project: Arc<Mutex<Option<Project>>>,

    /// Configuration settings for the language server
    pub(crate) settings: Arc<Mutex<Settings>>,

    /// Shared inspector for executing Python queries
    pub(crate) inspector: Arc<Inspector>,

    pub(crate) storage: salsa::Storage<Self>,

    // The logs are only used for testing and demonstrating reuse:
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) logs: Arc<Mutex<Option<Vec<String>>>>,
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

use std::sync::Arc;
use std::sync::Mutex;

use camino::Utf8Path;
use djls_project::Db as ProjectDb;
use djls_project::ModelGraph;
use djls_project::Project;
use djls_semantic::Db as SemanticDb;
use djls_semantic::FilterAritySpecs;
use djls_semantic::TagSpecs;
use djls_semantic::builtin_tag_specs;
use djls_source::Db as SourceDb;
use djls_source::File;
use djls_source::FileStatus;
use djls_source::FileSystem;
use djls_source::InMemoryFileSystem;
use djls_source::OsFileSystem;
use djls_source::SourceFiles;
use djls_source::path_to_file;
use salsa::Database;
use salsa::EventKind;

#[derive(Clone, Default)]
pub struct SalsaEventLog {
    events: Arc<Mutex<Vec<salsa::Event>>>,
}

impl SalsaEventLog {
    /// Drain and return all captured Salsa events.
    ///
    /// # Panics
    ///
    /// Panics if another test has poisoned the event log lock.
    #[must_use]
    pub fn take(&self) -> Vec<salsa::Event> {
        std::mem::take(
            &mut *self
                .events
                .lock()
                .expect("salsa event log lock should not be poisoned"),
        )
    }

    fn push(&self, event: salsa::Event) {
        self.events
            .lock()
            .expect("salsa event log lock should not be poisoned")
            .push(event);
    }

    /// Drain captured events and return the tracked functions that executed.
    #[must_use]
    pub fn take_will_execute_names(&self, db: &TestDatabase) -> Vec<String> {
        self.take()
            .into_iter()
            .filter_map(|event| match event.kind {
                EventKind::WillExecute { database_key } => Some(
                    db.ingredient_debug_name(database_key.ingredient_index())
                        .to_string(),
                ),
                _ => None,
            })
            .collect()
    }
}

#[salsa::db]
#[derive(Clone)]
pub struct TestDatabase {
    fs: Arc<Mutex<InMemoryFileSystem>>,
    files: SourceFiles,
    projectless_tag_specs: TagSpecs,
    projectless_filter_arity_specs: FilterAritySpecs,
    diagnostics_config: djls_conf::DiagnosticsConfig,
    project: Option<Project>,
    storage: salsa::Storage<Self>,
}

impl Default for TestDatabase {
    fn default() -> Self {
        Self::new()
    }
}

impl TestDatabase {
    #[must_use]
    pub fn new() -> Self {
        Self::with_storage(salsa::Storage::default())
    }

    #[must_use]
    pub fn case_insensitive() -> Self {
        let mut db = Self::with_storage(salsa::Storage::default());
        db.fs = Arc::new(Mutex::new(InMemoryFileSystem::case_insensitive()));
        db
    }

    #[must_use]
    pub fn with_event_log(event_log: SalsaEventLog) -> Self {
        Self::with_storage(salsa::Storage::new(Some(Box::new(move |event| {
            event_log.push(event);
        }))))
    }

    fn with_storage(storage: salsa::Storage<Self>) -> Self {
        Self {
            fs: Arc::new(Mutex::new(InMemoryFileSystem::new())),
            files: SourceFiles::default(),
            projectless_tag_specs: builtin_tag_specs(),
            projectless_filter_arity_specs: FilterAritySpecs::new(),
            diagnostics_config: djls_conf::DiagnosticsConfig::default(),
            project: None,
            storage,
        }
    }

    #[must_use]
    pub fn with_projectless_tag_specs(mut self, specs: TagSpecs) -> Self {
        self.projectless_tag_specs = specs;
        self
    }

    #[must_use]
    pub fn with_projectless_filter_arity_specs(mut self, specs: FilterAritySpecs) -> Self {
        self.projectless_filter_arity_specs = specs;
        self
    }

    #[must_use]
    pub fn with_diagnostics_config(
        mut self,
        diagnostics_config: djls_conf::DiagnosticsConfig,
    ) -> Self {
        self.diagnostics_config = diagnostics_config;
        self
    }

    /// Add an in-memory file to the test filesystem.
    ///
    /// # Panics
    ///
    /// Panics if another test has poisoned the in-memory filesystem lock.
    pub fn add_file(&self, path: &str, content: &str) {
        self.fs
            .lock()
            .expect("in-memory filesystem lock should not be poisoned")
            .add_file(path.into(), content.to_string());
    }

    /// Remove an in-memory file from the test filesystem.
    ///
    /// # Panics
    ///
    /// Panics if another test has poisoned the in-memory filesystem lock.
    pub fn remove_file(&self, path: &str) {
        self.fs
            .lock()
            .expect("in-memory filesystem lock should not be poisoned")
            .remove_file(Utf8Path::new(path));
    }

    pub fn set_project(&mut self, project: Project) {
        self.project = Some(project);
    }

    /// Return an existing fixture file from the test filesystem.
    ///
    /// # Panics
    ///
    /// Panics if the fixture has not been added to the in-memory filesystem.
    #[must_use]
    pub fn file(&self, path: &Utf8Path) -> File {
        path_to_file(self, path).expect("test fixture file should exist; call add_file first")
    }

    #[must_use]
    pub(crate) fn create_file_with_revision(&self, path: &Utf8Path, revision: u64) -> File {
        debug_assert!(
            self.file_system().is_file(path),
            "fixture file should exist before creating tracked file: {path}"
        );
        let file = File::builder(path.to_owned(), revision, FileStatus::Exists)
            .durability(salsa::Durability::LOW)
            .path_durability(salsa::Durability::HIGH)
            .new(self);
        self.files.register_file(self, file);
        file
    }
}

#[salsa::db]
#[derive(Clone)]
pub struct OsTestDatabase {
    storage: salsa::Storage<Self>,
    fs: Arc<dyn FileSystem>,
    files: SourceFiles,
    project: Option<Project>,
}

impl Default for OsTestDatabase {
    fn default() -> Self {
        Self::new()
    }
}

impl OsTestDatabase {
    #[must_use]
    pub fn new() -> Self {
        Self::with_file_system(Arc::new(OsFileSystem::default()))
    }

    #[must_use]
    pub fn with_file_system(fs: Arc<dyn FileSystem>) -> Self {
        Self {
            storage: salsa::Storage::default(),
            fs,
            files: SourceFiles::default(),
            project: None,
        }
    }

    pub fn set_project(&mut self, project: Project) {
        self.project = Some(project);
    }
}

#[salsa::db]
impl salsa::Database for TestDatabase {}

#[salsa::db]
impl djls_source::Db for TestDatabase {
    fn files(&self) -> &SourceFiles {
        &self.files
    }

    fn file_system(&self) -> &dyn FileSystem {
        self.fs.as_ref()
    }
}

#[salsa::db]
impl ProjectDb for TestDatabase {
    fn project(&self) -> Option<Project> {
        self.project
    }
}

#[salsa::db]
impl salsa::Database for OsTestDatabase {}

#[salsa::db]
impl djls_source::Db for OsTestDatabase {
    fn files(&self) -> &SourceFiles {
        &self.files
    }

    fn file_system(&self) -> &dyn FileSystem {
        self.fs.as_ref()
    }
}

#[salsa::db]
impl ProjectDb for OsTestDatabase {
    fn project(&self) -> Option<Project> {
        self.project
    }
}

#[salsa::db]
impl SemanticDb for TestDatabase {
    fn projectless_tag_specs(&self) -> &TagSpecs {
        &self.projectless_tag_specs
    }

    fn diagnostics_config(&self) -> djls_conf::DiagnosticsConfig {
        self.diagnostics_config.clone()
    }

    fn projectless_filter_arity_specs(&self) -> &FilterAritySpecs {
        &self.projectless_filter_arity_specs
    }

    fn model_graph(&self) -> &ModelGraph {
        ModelGraph::empty_ref()
    }
}

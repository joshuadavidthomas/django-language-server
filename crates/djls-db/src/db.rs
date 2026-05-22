//! Concrete Salsa database implementation for the Django Language Server.
//!
//! This module provides the concrete [`DjangoDatabase`] that implements all
//! the database traits from source, semantic, and project crates. This follows
//! Ruff's architecture pattern where the concrete database lives at the top level.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::Settings;
use djls_project::Db as LoadingDb;
use djls_project::Project as ProjectFacts;
use djls_project::ProjectDiscovery;
use djls_project::ProjectDiscoveryApplyResult;
use djls_project::ProjectDiscoveryIssue;
use djls_project::ProjectDiscoveryIssues;
use djls_project::ProjectDiscoverySet;
use djls_project::ProjectDiscoverySetData;
use djls_project::ProjectSourceFilesApplyResult;
use djls_project::ProjectSourceFilesMaterializationPatch;
use djls_project::ProjectSourceFilesUpdate;
use djls_project::ReadyProjectSourceFiles;
use djls_project::SourceFileHandleChanges;
use djls_project::SourceFileMaterializationIssue;
use djls_project::SourceFileSetMaterialized;
use djls_semantic::compute_filter_arity_specs;
use djls_semantic::compute_model_graph;
use djls_semantic::compute_tag_specs;
use djls_semantic::Db as SemanticDb;
use djls_semantic::Project;
use djls_semantic::ProjectDb;
use djls_semantic::ProjectIntrospector;
use djls_semantic::TagSpecs;
use djls_semantic::TemplateLibraries;
use djls_source::Db as SourceDb;
use djls_source::LoadedSourceFile;
use djls_source::SourceFileSet;
use djls_source::SourceFileSetData;
use djls_source::SourceFileSetInvariantError;
use djls_source::SourceFiles;
use djls_source::SourceRootEntry;
use djls_workspace::FileSystem;

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

    /// The single project for this database instance
    pub(crate) project: Arc<Mutex<Option<Project>>>,

    /// Configuration settings for the language server
    pub(crate) settings: Arc<Mutex<Settings>>,

    /// Shared introspector for external project facts.
    pub(crate) project_introspector: Arc<ProjectIntrospector>,

    /// Stable Salsa-visible project facts root.
    pub(crate) project_facts: Arc<OnceLock<ProjectFacts>>,

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

        let db = Self {
            fs: Arc::new(InMemoryFileSystem::new()),
            files: SourceFiles::default(),
            project: Arc::new(Mutex::new(None)),
            settings: Arc::new(Mutex::new(Settings::default())),
            project_introspector: Arc::new(ProjectIntrospector::new()),
            project_facts: Arc::new(OnceLock::new()),
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
        };
        let project = ProjectFacts::virtual_project(&db);
        db.project_facts
            .set(project)
            .expect("project facts should initialize once");
        db
    }
}

impl DjangoDatabase {
    /// Create a new [`DjangoDatabase`] with the given file system handle.
    pub fn new(file_system: Arc<dyn FileSystem>, settings: &Settings) -> Self {
        let db = Self {
            fs: file_system,
            files: SourceFiles::default(),
            project: Arc::new(Mutex::new(None)),
            settings: Arc::new(Mutex::new(settings.clone())),
            project_introspector: Arc::new(ProjectIntrospector::new()),
            project_facts: Arc::new(OnceLock::new()),
            storage: salsa::Storage::new(None),
            #[cfg(test)]
            logs: Arc::new(Mutex::new(None)),
        };
        let project = ProjectFacts::virtual_project(&db);
        db.project_facts
            .set(project)
            .expect("project facts should initialize once");
        db
    }

    /// Bootstrap the legacy single-project input for project-aware callers.
    ///
    /// LSP protocol startup intentionally does not call this. Batch/project-aware
    /// entrypoints such as `djls check` can call it explicitly until the
    /// root-scoped project loading graph replaces the old `Project` input.
    pub fn bootstrap_project(&mut self, root: &Utf8Path, settings: &Settings) {
        let project = Project::bootstrap(self, root, settings);
        *self.project.lock().unwrap() = Some(project);
    }

    pub fn apply_project_discovery_data(
        &mut self,
        data: ProjectDiscoverySetData,
    ) -> ProjectDiscoveryApplyResult {
        if data.roots().is_empty() {
            let issues = ProjectDiscoveryIssues::new(vec![ProjectDiscoveryIssue::NoWorkspaceRoots])
                .expect("no workspace roots issue should be non-empty");
            let discovery = ProjectDiscovery::Unavailable { issues };
            LoadingDb::set_project_discovery(self, discovery.clone());
            return ProjectDiscoveryApplyResult::Unavailable(discovery);
        }

        let current = LoadingDb::project(self).discovery(self).clone();
        let has_issues = data.roots().iter().any(|root| !root.issues().is_empty());
        if project_discovery_matches_data(self, &current, &data) {
            return ProjectDiscoveryApplyResult::Applied {
                discovery: current,
                has_issues,
            };
        }

        let roots = data
            .roots()
            .iter()
            .map(|root| {
                djls_project::RootDiscoveryInput::new(
                    self,
                    root.root().clone(),
                    root.interpreter().cloned(),
                    root.settings_module_seed().cloned(),
                    root.configured_environment_seeds().to_vec(),
                    root.pythonpath().to_vec(),
                    root.env_vars().clone(),
                    root.issues().to_vec(),
                )
            })
            .collect();
        let set = ProjectDiscoverySet::new(roots)
            .expect("non-empty discovery data should construct discovery set");
        let discovery = ProjectDiscovery::Ready(set);
        LoadingDb::set_project_discovery(self, discovery.clone());
        ProjectDiscoveryApplyResult::Applied {
            discovery,
            has_issues,
        }
    }

    pub fn apply_project_source_files(
        &mut self,
        update: ProjectSourceFilesUpdate,
    ) -> ProjectSourceFilesApplyResult {
        let previous = self.current_ready_project_source_files();
        let materialized =
            self.materialize_source_file_set_from(previous.as_ref(), update.materialization());
        djls_project::finalize_project_source_files(self, previous, update, materialized)
    }

    pub fn materialize_source_file_set(
        &mut self,
        patch: &ProjectSourceFilesMaterializationPatch,
    ) -> SourceFileSetMaterialized {
        let previous = self.current_ready_project_source_files();
        self.materialize_source_file_set_from(previous.as_ref(), patch)
    }

    fn materialize_source_file_set_from(
        &mut self,
        previous: Option<&ReadyProjectSourceFiles>,
        patch: &ProjectSourceFilesMaterializationPatch,
    ) -> SourceFileSetMaterialized {
        let previous_data = previous.map(|files| files.merged().data(self).clone());
        let removed_roots = patch.removed_roots().iter().collect::<BTreeSet<_>>();
        let removed_files = patch.removed_files().iter().collect::<BTreeSet<_>>();
        let mut roots = previous_data
            .as_ref()
            .map(|data| {
                data.roots()
                    .iter()
                    .filter(|entry| !removed_roots.contains(entry.root().id()))
                    .cloned()
                    .map(|entry| (entry.root().id().clone(), entry))
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        roots.extend(
            patch
                .changed_roots()
                .iter()
                .cloned()
                .map(|entry| (entry.root().id().clone(), entry)),
        );

        for entry in roots.values() {
            self.files
                .try_add_root(entry.root().path().to_owned(), entry.root().kind());
        }

        let mut loaded = previous_data
            .as_ref()
            .map(|data| {
                data.files()
                    .iter()
                    .filter(|file| {
                        !removed_roots.contains(file.root())
                            && !removed_files.contains(&file.path().to_owned())
                    })
                    .cloned()
                    .map(|file| (file.path().to_owned(), file))
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        let previous_handles = loaded
            .iter()
            .map(|(path, file)| (path.clone(), file.file()))
            .collect::<BTreeMap<_, _>>();

        let mut preserved = 0;
        let mut created = 0;
        for discovered in patch.upserted_files() {
            let file = if let Some(file) = previous_handles.get(discovered.path()) {
                preserved += 1;
                *file
            } else {
                created += 1;
                self.get_or_create_file(discovered.path())
            };
            loaded.insert(
                discovered.path().to_owned(),
                LoadedSourceFile::from_discovered(discovered.clone(), file),
            );
        }

        let removed = previous_data
            .as_ref()
            .map(|data| {
                data.files()
                    .iter()
                    .filter(|file| {
                        removed_roots.contains(file.root())
                            || removed_files.contains(&file.path().to_owned())
                    })
                    .count()
            })
            .unwrap_or(0);
        let roots = roots.into_values().collect::<Vec<SourceRootEntry>>();
        let files = loaded.into_values().collect::<Vec<_>>();
        let (data, issues) = match SourceFileSetData::new(roots, files) {
            Ok(data) => (data, Vec::new()),
            Err(error) => (
                SourceFileSetData::default(),
                vec![materialization_issue_from_invariant_error(error)],
            ),
        };
        let source_file_set = SourceFileSet::new(self, data);
        SourceFileSetMaterialized::new(
            source_file_set,
            SourceFileHandleChanges::new(preserved, created, removed),
            issues,
        )
    }

    fn current_ready_project_source_files(&self) -> Option<ReadyProjectSourceFiles> {
        LoadingDb::project(self).source_inventory(self).ready()
    }
}

fn project_discovery_matches_data(
    db: &dyn djls_project::Db,
    discovery: &ProjectDiscovery,
    data: &ProjectDiscoverySetData,
) -> bool {
    let ProjectDiscovery::Ready(discovery) = discovery else {
        return false;
    };
    let roots = discovery.roots();
    roots.len() == data.roots().len()
        && roots.iter().zip(data.roots()).all(|(input, data)| {
            input.root(db) == data.root()
                && input.interpreter(db).as_ref() == data.interpreter()
                && input.settings_module_seed(db).as_ref() == data.settings_module_seed()
                && input.configured_environment_seeds(db) == data.configured_environment_seeds()
                && input.pythonpath(db) == data.pythonpath()
                && input.env_vars(db) == data.env_vars()
                && input.issues(db) == data.issues()
        })
}

fn materialization_issue_from_invariant_error(
    error: SourceFileSetInvariantError,
) -> SourceFileMaterializationIssue {
    match error {
        SourceFileSetInvariantError::UnknownFileRoot { root, .. }
        | SourceFileSetInvariantError::DuplicateRootId { root, .. } => {
            SourceFileMaterializationIssue::MissingRoot { root }
        }
        SourceFileSetInvariantError::DuplicateFile { path, .. } => {
            SourceFileMaterializationIssue::MaterializationFailed {
                path,
                error_kind: std::io::ErrorKind::AlreadyExists,
            }
        }
    }
}

#[salsa::db]
impl salsa::Database for DjangoDatabase {}

#[salsa::db]
impl djls_project::Db for DjangoDatabase {
    fn project(&self) -> ProjectFacts {
        *self
            .project_facts
            .get()
            .expect("project facts should be initialized")
    }
}

#[salsa::db]
impl SourceDb for DjangoDatabase {
    fn files(&self) -> &SourceFiles {
        &self.files
    }

    fn read_file(&self, path: &Utf8Path) -> std::io::Result<String> {
        self.fs.read_to_string(path)
    }
}

#[salsa::db]
impl SemanticDb for DjangoDatabase {
    fn tag_specs(&self) -> &TagSpecs {
        if let Some(project) = ProjectDb::project(self) {
            compute_tag_specs(self, project)
        } else {
            static DEFAULT: std::sync::LazyLock<TagSpecs> =
                std::sync::LazyLock::new(djls_semantic::builtin_tag_specs);
            &DEFAULT
        }
    }

    fn template_dirs(&self) -> Option<Vec<Utf8PathBuf>> {
        ProjectDb::project(self).and_then(|project| {
            project
                .template_dirs(self)
                .as_known()
                .map(<[Utf8PathBuf]>::to_vec)
        })
    }

    fn diagnostics_config(&self) -> djls_conf::DiagnosticsConfig {
        self.settings().diagnostics().clone()
    }

    fn template_libraries(&self) -> &TemplateLibraries {
        ProjectDb::project(self).map_or(TemplateLibraries::empty_ref(), |project| {
            project.template_libraries(self)
        })
    }

    fn filter_arity_specs(&self) -> &djls_semantic::FilterAritySpecs {
        ProjectDb::project(self).map_or(djls_semantic::FilterAritySpecs::empty_ref(), |project| {
            compute_filter_arity_specs(self, project)
        })
    }

    fn model_graph(&self) -> &djls_semantic::ModelGraph {
        ProjectDb::project(self).map_or(djls_semantic::ModelGraph::empty_ref(), |project| {
            compute_model_graph(self, project)
        })
    }
}

#[salsa::db]
impl ProjectDb for DjangoDatabase {
    fn project(&self) -> Option<Project> {
        *self.project.lock().unwrap()
    }

    fn project_introspector(&self) -> Arc<ProjectIntrospector> {
        self.project_introspector.clone()
    }
}

#[cfg(test)]
mod source_file_set_tests {
    use camino::Utf8PathBuf;
    use djls_project::build_source_roots;
    use djls_project::first_party_discovery_files_request;
    use djls_project::first_party_source_files_load_request;
    use djls_project::merge_first_party_source_file_patch;
    use djls_project::Db as LoadingDb;
    use djls_project::FirstPartySourceFilePatch;
    use djls_project::ProjectSourceFilesApplyResult;
    use djls_project::ProjectSourceFilesIssue;
    use djls_project::ProjectSourceInventory;
    use djls_source::Db as SourceDb;
    use djls_workspace::load_files_for_roots;

    use super::DjangoDatabase;

    fn utf8(path: &std::path::Path) -> Utf8PathBuf {
        Utf8PathBuf::from_path_buf(path.to_path_buf()).unwrap()
    }

    fn first_party_update(
        current: Option<&djls_project::ReadyProjectSourceFiles>,
        roots: Vec<Utf8PathBuf>,
    ) -> djls_project::ProjectSourceFilesUpdate {
        let plan = build_source_roots(roots);
        let (root_issues, request) =
            first_party_discovery_files_request(first_party_source_files_load_request(plan));
        let result = load_files_for_roots(request);
        merge_first_party_source_file_patch(
            current,
            FirstPartySourceFilePatch::first_party(root_issues, result),
        )
    }

    #[test]
    fn source_file_set_materialization_preserves_unchanged_file_handles() {
        let dir = tempfile::tempdir().unwrap();
        let root = utf8(dir.path());
        let file_path = root.join("templates/index.html");
        std::fs::create_dir_all(file_path.parent().unwrap()).unwrap();
        std::fs::write(&file_path, "").unwrap();
        let mut db = DjangoDatabase::default();

        let update = first_party_update(None, vec![root.clone()]);
        let materialized = db.materialize_source_file_set(update.materialization());
        assert_eq!(materialized.handle_changes().created(), 1);
        assert_eq!(materialized.handle_changes().preserved(), 0);
        let applied =
            djls_project::finalize_project_source_files(&mut db, None, update, materialized);
        let ProjectSourceFilesApplyResult::Applied(applied) = applied else {
            panic!("first materialization should apply");
        };
        let first_handle = db.get_file(file_path.as_path()).unwrap();

        let update = first_party_update(Some(applied.files()), vec![root]);
        let materialized = db.materialize_source_file_set(update.materialization());
        assert_eq!(materialized.handle_changes().created(), 0);
        assert_eq!(materialized.handle_changes().preserved(), 0);
        let applied = djls_project::finalize_project_source_files(
            &mut db,
            Some(applied.files().clone()),
            update,
            materialized,
        );
        let ProjectSourceFilesApplyResult::Applied(applied) = applied else {
            panic!("second materialization should apply");
        };

        assert_eq!(db.get_file(file_path.as_path()), Some(first_handle));
        assert_eq!(
            applied.files().merged().data(&db).files()[0].file(),
            first_handle
        );
    }

    #[test]
    fn source_file_set_materialization_counts_removed_root_files_once() {
        let first_dir = tempfile::tempdir().unwrap();
        let first_root = utf8(first_dir.path());
        let removed_file = first_root.join("gone.py");
        std::fs::write(&removed_file, "").unwrap();
        let mut db = DjangoDatabase::default();

        let update = first_party_update(None, vec![first_root]);
        let applied = db.apply_project_source_files(update);
        let ProjectSourceFilesApplyResult::Applied(applied) = applied else {
            panic!("first materialization should apply");
        };

        let second_dir = tempfile::tempdir().unwrap();
        let second_root = utf8(second_dir.path());
        let update = first_party_update(Some(applied.files()), vec![second_root]);
        let materialized = db.materialize_source_file_set(update.materialization());

        assert_eq!(materialized.handle_changes().removed(), 1);
    }

    #[test]
    fn terminal_issue_preserves_previous_ready_source_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = utf8(dir.path());
        std::fs::write(root.join("models.py"), "").unwrap();
        let mut db = DjangoDatabase::default();

        let update = first_party_update(None, vec![root]);
        let applied = db.apply_project_source_files(update);
        let ProjectSourceFilesApplyResult::Applied(applied) = applied else {
            panic!("initial materialization should apply");
        };

        let missing = utf8(tempfile::tempdir().unwrap().path()).join("missing");
        let update = first_party_update(Some(applied.files()), vec![missing]);
        let result = db.apply_project_source_files(update);

        let ProjectSourceFilesApplyResult::Unavailable { previous, .. } = result else {
            panic!("missing root should be unavailable");
        };
        assert_eq!(previous, Some(applied.files().clone()));
        assert_eq!(
            LoadingDb::project(&db).source_inventory(&db),
            ProjectSourceInventory::Ready(applied.files().clone())
        );
    }

    #[test]
    fn source_file_set_roundtrip_finalizes_ready_source_inventory() {
        let dir = tempfile::tempdir().unwrap();
        let root = utf8(dir.path());
        std::fs::write(root.join("models.py"), "").unwrap();
        let mut db = DjangoDatabase::default();

        let update = first_party_update(None, vec![root]);
        let transition = update.applied_transition().clone();
        let applied = db.apply_project_source_files(update);
        let ProjectSourceFilesApplyResult::Applied(applied) = applied else {
            panic!("materialization should apply");
        };

        assert_eq!(applied.transition(), &transition);
        assert_eq!(applied.files().summary(&db).included_files(), 1);
        assert_eq!(
            LoadingDb::project(&db).source_inventory(&db),
            ProjectSourceInventory::Ready(applied.files().clone())
        );
    }

    #[test]
    fn source_file_set_terminal_issue_updates_query_visible_inventory_when_no_prior_facts() {
        let dir = tempfile::tempdir().unwrap();
        let missing = utf8(dir.path()).join("missing");
        let mut db = DjangoDatabase::default();

        let update = first_party_update(None, vec![missing.clone()]);
        let result = db.apply_project_source_files(update);
        let ProjectSourceFilesApplyResult::Unavailable { issue, .. } = result else {
            panic!("missing root should be unavailable");
        };

        assert!(matches!(
            issue,
            ProjectSourceFilesIssue::MissingRoot { ref path, .. } if *path == missing
        ));
        assert_eq!(
            LoadingDb::project(&db).source_inventory(&db),
            ProjectSourceInventory::Unavailable { issue }
        );
    }
}

#[cfg(test)]
mod project_discovery_tests {
    use camino::Utf8PathBuf;
    use djls_project::Db as ProjectDb;
    use djls_project::ProjectDiscovery;
    use djls_project::ProjectDiscoveryApplyResult;
    use djls_project::ProjectDiscoveryIssue;
    use djls_project::ProjectDiscoverySetData;
    use djls_project::ProjectEnvVars;
    use djls_project::RootDiscoveryData;

    use super::DjangoDatabase;

    fn root_data(path: &str) -> RootDiscoveryData {
        RootDiscoveryData::new(
            Utf8PathBuf::from(path),
            None,
            None,
            Vec::new(),
            Vec::new(),
            ProjectEnvVars::default(),
            Vec::new(),
        )
    }

    #[test]
    fn apply_project_discovery_data_sets_ready_project_fact() {
        let mut db = DjangoDatabase::default();
        let result =
            db.apply_project_discovery_data(ProjectDiscoverySetData::new(vec![root_data(
                "/workspace",
            )]));

        assert!(matches!(
            result,
            ProjectDiscoveryApplyResult::Applied { .. }
        ));
        let ProjectDiscovery::Ready(discovery) = ProjectDb::project(&db).discovery(&db) else {
            panic!("discovery should be ready");
        };
        assert_eq!(discovery.roots().len(), 1);
        assert_eq!(
            discovery.roots()[0].root(&db),
            &Utf8PathBuf::from("/workspace")
        );
    }

    #[test]
    fn empty_project_discovery_apply_replaces_previous_ready_with_unavailable() {
        let mut db = DjangoDatabase::default();
        db.apply_project_discovery_data(ProjectDiscoverySetData::new(vec![root_data(
            "/workspace",
        )]));
        let result = db.apply_project_discovery_data(ProjectDiscoverySetData::new(Vec::new()));

        let ProjectDiscoveryApplyResult::Unavailable(ProjectDiscovery::Unavailable { issues }) =
            result
        else {
            panic!("empty discovery data should be unavailable");
        };
        assert_eq!(
            issues.as_slice(),
            &[ProjectDiscoveryIssue::NoWorkspaceRoots]
        );
        let ProjectDiscovery::Unavailable { issues } = ProjectDb::project(&db).discovery(&db)
        else {
            panic!("empty discovery data should replace previous ready facts");
        };
        assert_eq!(
            issues.as_slice(),
            &[ProjectDiscoveryIssue::NoWorkspaceRoots]
        );
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
    use djls_semantic::Db as SemanticDb;
    use djls_semantic::Interpreter;
    use djls_semantic::Knowledge;
    use djls_semantic::Project;
    use djls_semantic::TemplateDirs;
    use djls_semantic::TemplateLibraries;
    use djls_source::SourceFiles;
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
            files: SourceFiles::default(),
            project: Arc::new(Mutex::new(None)),
            settings: Arc::new(Mutex::new(settings.clone())),
            project_introspector: Arc::new(djls_semantic::ProjectIntrospector::new()),
            project_facts: Arc::new(std::sync::OnceLock::new()),
            storage: salsa::Storage::new(Some(Box::new({
                let log = event_log.clone();
                move |event| {
                    log.events.lock().unwrap().push(event);
                }
            }))),
            logs: Arc::new(Mutex::new(None)),
        };
        let project_facts = djls_project::Project::fixture_unavailable(&db);
        db.project_facts
            .set(project_facts)
            .expect("project facts should initialize once");

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
            Vec::new(),
            TemplateDirs::Unknown,
            settings.tagspecs().clone(),
            TemplateLibraries::default(),
            rustc_hash::FxHashMap::default(),
            rustc_hash::FxHashMap::default(),
            rustc_hash::FxHashMap::default(),
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

        let response = djls_semantic::TemplateLibrarySnapshot {
            symbols: Vec::new(),
            libraries: BTreeMap::new(),
            builtins: Vec::new(),
        };

        let new_libraries = TemplateLibraries::default().apply_active_snapshot(Some(response));

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
            files: SourceFiles::default(),
            project: Arc::new(Mutex::new(None)),
            settings: Arc::new(Mutex::new(settings.clone())),
            project_introspector: Arc::new(djls_semantic::ProjectIntrospector::new()),
            project_facts: Arc::new(std::sync::OnceLock::new()),
            storage: salsa::Storage::new(Some(Box::new({
                let log = event_log.clone();
                move |event| {
                    log.events.lock().unwrap().push(event);
                }
            }))),
            logs: Arc::new(Mutex::new(None)),
        };
        let project_facts = djls_project::Project::fixture_unavailable(&db);
        db.project_facts
            .set(project_facts)
            .expect("project facts should initialize once");

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

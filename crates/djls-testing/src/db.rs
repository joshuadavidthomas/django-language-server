use std::io;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_project::Db as ProjectDb;
use djls_project::Project;
use djls_semantic::Db as SemanticDb;
use djls_semantic::FilterAritySpecs;
use djls_semantic::TagSpecs;
use djls_semantic::builtin_tag_specs;
use djls_source::CaseSensitivity;
use djls_source::ChangeEvent;
use djls_source::File;
use djls_source::FileStatus;
use djls_source::FileSystem;
use djls_source::InMemoryFileSystem;
use djls_source::OsFileSystem;
use djls_source::RootWalk;
use djls_source::SourceChanges;
use djls_source::SourceFiles;
use djls_source::Utf8PathClean;
use djls_source::WalkOptions;
use djls_source::path_to_file;
use salsa::Database;
use salsa::EventKind;

#[derive(Clone, Default)]
pub struct SalsaEventLog {
    events: Arc<Mutex<Vec<salsa::Event>>>,
    poisoned: Arc<AtomicBool>,
}

impl SalsaEventLog {
    /// Drain and return all captured Salsa events.
    pub fn take(&self) -> anyhow::Result<Vec<salsa::Event>> {
        if self.poisoned.load(Ordering::Acquire) {
            anyhow::bail!("salsa event log lock was poisoned while recording an event");
        }
        let mut events = self
            .events
            .lock()
            .map_err(|_error| anyhow::anyhow!("salsa event log lock is poisoned"))?;
        Ok(std::mem::take(&mut *events))
    }

    fn push(&self, event: salsa::Event) {
        match self.events.lock() {
            Ok(mut events) => events.push(event),
            Err(_) => self.poisoned.store(true, Ordering::Release),
        }
    }

    /// Drain captured events and return the tracked functions that executed.
    pub fn take_will_execute_names(&self, db: &TestDatabase) -> anyhow::Result<Vec<String>> {
        Ok(self
            .take()?
            .into_iter()
            .filter_map(|event| match event.kind {
                EventKind::WillExecute { database_key } => Some(
                    db.ingredient_debug_name(database_key.ingredient_index())
                        .to_string(),
                ),
                EventKind::DidValidateMemoizedValue { .. }
                | EventKind::WillBlockOn { .. }
                | EventKind::WillIterateCycle { .. }
                | EventKind::DidFinalizeCycle { .. }
                | EventKind::WillCheckCancellation
                | EventKind::DidSetCancellationFlag
                | EventKind::WillDiscardStaleOutput { .. }
                | EventKind::DidDiscard { .. }
                | EventKind::DidDiscardAccumulated { .. }
                | EventKind::DidInternValue { .. }
                | EventKind::DidReuseInternedValue { .. }
                | EventKind::DidValidateInternedValue { .. } => None,
            })
            .collect())
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
    pub fn add_file(&self, path: &str, content: &str) -> anyhow::Result<()> {
        self.fs
            .lock()
            .map_err(|_error| anyhow::anyhow!("in-memory filesystem lock is poisoned"))?
            .add_file(path.into(), content.to_string());
        Ok(())
    }

    /// Remove an in-memory file from the test filesystem.
    pub fn remove_file(&self, path: &str) -> anyhow::Result<()> {
        self.fs
            .lock()
            .map_err(|_error| anyhow::anyhow!("in-memory filesystem lock is poisoned"))?
            .remove_file(Utf8Path::new(path));
        Ok(())
    }

    pub fn set_project(&mut self, project: Project) {
        self.project = Some(project);
    }

    /// Return an existing fixture file from the test filesystem.
    pub fn file(&self, path: &Utf8Path) -> Result<File, djls_source::FileError> {
        path_to_file(self, path)
    }

    /// Register a fresh file identity with an explicit revision.
    pub fn create_file_with_revision(
        &self,
        path: &Utf8Path,
        revision: u64,
    ) -> Result<File, djls_source::FileError> {
        self.file(path)?;
        let file = File::builder(path.to_owned(), revision, FileStatus::Exists)
            .durability(salsa::Durability::LOW)
            .path_durability(salsa::Durability::HIGH)
            .new(self);
        self.files.register_file(self, file);
        Ok(file)
    }
}

/// Filesystem for tests that overlays mutable in-memory files on bounded disk sources.
#[derive(Clone)]
struct LayeredFileSystem {
    memory: Arc<Mutex<InMemoryFileSystem>>,
    disk: Arc<dyn FileSystem>,
    disk_roots: Arc<[Utf8PathBuf]>,
}

impl LayeredFileSystem {
    fn new(
        memory: Arc<Mutex<InMemoryFileSystem>>,
        disk: Arc<dyn FileSystem>,
        disk_roots: impl IntoIterator<Item = Utf8PathBuf>,
    ) -> Self {
        let mut disk_roots = disk_roots
            .into_iter()
            .map(|root| root.clean())
            .collect::<Vec<_>>();
        disk_roots.sort();
        disk_roots.dedup();
        Self {
            memory,
            disk,
            disk_roots: disk_roots.into(),
        }
    }

    fn disk_path(&self, path: &Utf8Path) -> Option<Utf8PathBuf> {
        let path = path.clean();
        self.disk_roots
            .iter()
            .any(|root| path.starts_with(root))
            .then_some(path)
    }
}

impl FileSystem for LayeredFileSystem {
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
        match self.memory.read_to_string(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound && !self.memory.exists(path) => {
                self.disk_path(path)
                    .map_or(Err(error), |path| self.disk.read_to_string(&path))
            }
            result => result,
        }
    }

    fn exists(&self, path: &Utf8Path) -> bool {
        self.memory.exists(path)
            || self
                .disk_path(path)
                .is_some_and(|path| self.disk.exists(&path))
    }

    fn is_file(&self, path: &Utf8Path) -> bool {
        if self.memory.exists(path) {
            self.memory.is_file(path)
        } else {
            self.disk_path(path)
                .is_some_and(|path| self.disk.is_file(&path))
        }
    }

    fn is_dir(&self, path: &Utf8Path) -> bool {
        if self.memory.exists(path) {
            self.memory.is_dir(path)
        } else {
            self.disk_path(path)
                .is_some_and(|path| self.disk.is_dir(&path))
        }
    }

    fn case_sensitivity(&self) -> CaseSensitivity {
        if self.disk_roots.is_empty() {
            self.memory.case_sensitivity()
        } else {
            self.disk.case_sensitivity()
        }
    }

    fn path_exists_case_sensitive(&self, path: &Utf8Path, prefix: &Utf8Path) -> bool {
        if self.memory.exists(path) {
            self.memory.path_exists_case_sensitive(path, prefix)
        } else {
            self.disk_path(path).is_some_and(|path| {
                self.disk_path(prefix)
                    .is_some_and(|prefix| self.disk.path_exists_case_sensitive(&path, &prefix))
            })
        }
    }

    fn walk_root(&self, root: &Utf8Path, options: &WalkOptions) -> RootWalk {
        let memory = self.memory.walk_root(root, options);
        let Some(disk_root) = self.disk_path(root) else {
            return memory;
        };

        let disk = self.disk.walk_root(&disk_root, options);
        let (mut memory_entries, mut memory_issues) = match memory {
            RootWalk::Directory { entries, issues } => (entries, issues),
            RootWalk::Missing | RootWalk::Inaccessible(_) => return disk,
            RootWalk::File(entry) => return RootWalk::File(entry),
        };

        match disk {
            RootWalk::Directory { entries, issues } => {
                memory_entries.extend(entries);
                memory_issues.extend(issues);
            }
            RootWalk::Inaccessible(issue) => memory_issues.push(issue),
            RootWalk::Missing | RootWalk::File(_) => {}
        }
        memory_entries.sort_by(|left, right| left.path.cmp(&right.path));
        memory_entries.dedup_by(|left, right| left.path == right.path);
        RootWalk::Directory {
            entries: memory_entries,
            issues: memory_issues,
        }
    }
}

#[salsa::db]
#[derive(Clone)]
pub struct OsTestDatabase {
    storage: salsa::Storage<Self>,
    fs: Arc<dyn FileSystem>,
    memory: Arc<Mutex<InMemoryFileSystem>>,
    files: SourceFiles,
    project: Option<Project>,
    projectless_tag_specs: TagSpecs,
    diagnostics_config: djls_conf::DiagnosticsConfig,
}

impl Default for OsTestDatabase {
    fn default() -> Self {
        Self::new()
    }
}

impl OsTestDatabase {
    /// Create a database whose filesystem contains only in-memory files.
    #[must_use]
    pub fn new() -> Self {
        Self::with_disk_roots([])
    }

    /// Create a database that may read from the given disk roots.
    #[must_use]
    pub fn with_disk_roots(disk_roots: impl IntoIterator<Item = Utf8PathBuf>) -> Self {
        Self::with_file_system(Arc::new(OsFileSystem::default()), disk_roots)
    }

    #[must_use]
    pub fn with_file_system(
        disk: Arc<dyn FileSystem>,
        disk_roots: impl IntoIterator<Item = Utf8PathBuf>,
    ) -> Self {
        Self::with_file_system_and_storage(disk, disk_roots, salsa::Storage::default())
    }

    /// Create a layered database that records Salsa events.
    #[must_use]
    pub fn with_file_system_and_event_log(
        disk: Arc<dyn FileSystem>,
        disk_roots: impl IntoIterator<Item = Utf8PathBuf>,
        event_log: SalsaEventLog,
    ) -> Self {
        Self::with_file_system_and_storage(
            disk,
            disk_roots,
            salsa::Storage::new(Some(Box::new(move |event| event_log.push(event)))),
        )
    }

    fn with_file_system_and_storage(
        disk: Arc<dyn FileSystem>,
        disk_roots: impl IntoIterator<Item = Utf8PathBuf>,
        storage: salsa::Storage<Self>,
    ) -> Self {
        let memory = Arc::new(Mutex::new(InMemoryFileSystem::new()));
        let fs = Arc::new(LayeredFileSystem::new(
            Arc::clone(&memory),
            disk,
            disk_roots,
        ));
        Self {
            storage,
            fs,
            memory,
            files: SourceFiles::default(),
            project: None,
            projectless_tag_specs: TagSpecs::default(),
            diagnostics_config: djls_conf::DiagnosticsConfig::default(),
        }
    }

    #[must_use]
    pub fn with_diagnostics_config(
        mut self,
        diagnostics_config: djls_conf::DiagnosticsConfig,
    ) -> Self {
        self.diagnostics_config = diagnostics_config;
        self
    }

    #[must_use]
    pub fn with_projectless_tag_specs(mut self, specs: TagSpecs) -> Self {
        self.projectless_tag_specs = specs;
        self
    }

    pub(crate) fn insert_fixture_file(&self, path: &str, content: &str) -> anyhow::Result<()> {
        // Fixture setup precedes root registration and queries, so no change event is needed.
        self.memory
            .lock()
            .map_err(|_error| anyhow::anyhow!("in-memory filesystem lock is poisoned"))?
            .add_file(path.into(), content.to_string());
        Ok(())
    }

    /// Add an in-memory file above the database's disk filesystem.
    pub fn add_file(&mut self, path: &str, content: &str) -> anyhow::Result<File> {
        let path = Utf8PathBuf::from(path);
        let was_visible = self.fs.is_file(&path);
        self.memory
            .lock()
            .map_err(|_error| anyhow::anyhow!("in-memory filesystem lock is poisoned"))?
            .add_file(path.clone(), content.to_string());
        let event = if was_visible {
            ChangeEvent::ContentChanged(path.clone())
        } else {
            ChangeEvent::BecameVisible(path.clone())
        };
        SourceChanges::new([event]).apply(self);
        Ok(path_to_file(self, &path)?)
    }

    /// Remove an in-memory file from the layered filesystem.
    pub fn remove_file(&mut self, path: &str) -> anyhow::Result<()> {
        let path = Utf8PathBuf::from(path);
        self.memory
            .lock()
            .map_err(|_error| anyhow::anyhow!("in-memory filesystem lock is poisoned"))?
            .remove_file(&path);
        SourceChanges::new([ChangeEvent::Deleted(path)]).apply(self);
        Ok(())
    }

    /// Return an existing file from the layered test filesystem.
    pub fn file(&self, path: &Utf8Path) -> Result<File, djls_source::FileError> {
        path_to_file(self, path)
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
impl SemanticDb for OsTestDatabase {
    fn projectless_tag_specs(&self) -> &TagSpecs {
        &self.projectless_tag_specs
    }

    fn diagnostics_config(&self) -> djls_conf::DiagnosticsConfig {
        self.diagnostics_config.clone()
    }

    fn projectless_filter_arity_specs(&self) -> &FilterAritySpecs {
        FilterAritySpecs::empty_ref()
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
}

#[cfg(test)]
mod tests {
    use djls_source::Db as _;

    use super::*;
    use crate::ProjectFixture;

    #[test]
    fn layered_filesystem_reads_disk_only_below_allowed_roots() {
        let mut disk = InMemoryFileSystem::new();
        disk.add_file("/allowed/disk.py".into(), "allowed".to_string());
        disk.add_file("/allowed/directory/child.py".into(), "child".to_string());
        disk.add_file("/blocked/disk.py".into(), "blocked".to_string());
        let mut db =
            OsTestDatabase::with_file_system(Arc::new(disk), [Utf8PathBuf::from("/allowed")]);
        db.add_file("/blocked/memory.py", "memory")
            .expect("memory file should be added");

        assert_eq!(
            db.file_system()
                .read_to_string(Utf8Path::new("/allowed/disk.py"))
                .expect("allowed disk file should be readable"),
            "allowed"
        );
        assert_eq!(
            db.file_system()
                .read_to_string(Utf8Path::new("/blocked/../allowed/disk.py"))
                .expect("disk should receive the cleaned allowed path"),
            "allowed"
        );
        assert!(db.file_system().is_dir(Utf8Path::new("/allowed")));
        assert!(!db.file_system().exists(Utf8Path::new("/blocked/disk.py")));
        assert!(!db.file_system().is_file(Utf8Path::new("/blocked/disk.py")));
        assert!(!db.file_system().path_exists_case_sensitive(
            Utf8Path::new("/allowed/disk.py"),
            Utf8Path::new("/blocked")
        ));
        assert_eq!(
            db.file_system()
                .read_to_string(Utf8Path::new("/allowed/../blocked/disk.py"))
                .expect_err("parent traversal must not escape a disk root")
                .kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(
            db.file_system()
                .read_to_string(Utf8Path::new("/blocked/memory.py"))
                .expect("memory file should be readable outside disk roots"),
            "memory"
        );

        let RootWalk::Directory { entries, issues } = db
            .file_system()
            .walk_root(Utf8Path::new("/allowed"), &WalkOptions::default())
        else {
            panic!("allowed disk root should be walkable");
        };
        assert!(issues.is_empty());
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].path, Utf8Path::new("/allowed/directory"));
        assert_eq!(
            entries[1].path,
            Utf8Path::new("/allowed/directory/child.py")
        );
        assert_eq!(entries[2].path, Utf8Path::new("/allowed/disk.py"));

        let RootWalk::Directory { entries, issues } = db
            .file_system()
            .walk_root(Utf8Path::new("/blocked"), &WalkOptions::default())
        else {
            panic!("memory directory should be walkable");
        };
        assert!(issues.is_empty());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, Utf8Path::new("/blocked/memory.py"));
    }

    #[test]
    fn project_fixture_installs_files_in_os_database() {
        let mut db = OsTestDatabase::new();
        let project = ProjectFixture::new("/project")
            .file("/project/module.py", "VALUE = 1\n")
            .install(&mut db)
            .expect("OS-backed project fixture should install");

        assert_eq!(db.project(), Some(project));
        assert_eq!(
            db.file_system()
                .read_to_string(Utf8Path::new("/project/module.py"))
                .expect("fixture file should be readable"),
            "VALUE = 1\n"
        );
    }

    #[test]
    fn new_os_test_database_is_memory_only() {
        let mut db = OsTestDatabase::new();
        let disk_file = Utf8Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert!(disk_file.is_file());
        assert!(!db.file_system().exists(&disk_file));

        db.add_file("/memory.py", "value = 1")
            .expect("memory file should be added");
        assert!(db.file_system().is_file(Utf8Path::new("/memory.py")));
        assert_eq!(
            db.file_system().case_sensitivity(),
            CaseSensitivity::CaseSensitive
        );
    }
}

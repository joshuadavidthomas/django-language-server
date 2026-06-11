use std::sync::Arc;
use std::sync::Mutex;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::Settings;
use djls_conf::TagSpecDef;
use djls_source::Db as _;
use djls_source::FileSystem;
use djls_source::InMemoryFileSystem;
use djls_source::SourceFiles;

use crate::Db as ProjectDb;
use crate::Interpreter;
use crate::Project;
use crate::resolve::SearchPaths;

#[salsa::db]
#[derive(Clone)]
pub(crate) struct TestDatabase {
    storage: salsa::Storage<Self>,
    fs: Arc<Mutex<InMemoryFileSystem>>,
    files: SourceFiles,
    project: Option<Project>,
}

impl TestDatabase {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            storage: salsa::Storage::default(),
            fs: Arc::new(Mutex::new(InMemoryFileSystem::new())),
            files: SourceFiles::default(),
            project: None,
        }
    }

    pub(crate) fn add_file(&self, path: &str, content: &str) {
        self.fs
            .lock()
            .unwrap()
            .add_file(path.into(), content.to_string());
    }

    pub(crate) fn remove_file(&self, path: &str) {
        self.fs.lock().unwrap().remove_file(Utf8Path::new(path));
    }

    pub(crate) fn set_project(&mut self, project: Project) {
        self.project = Some(project);
    }
}

pub(crate) struct ProjectFixture {
    root: Utf8PathBuf,
    files: Vec<(Utf8PathBuf, String)>,
    django_settings_module: Option<String>,
    pythonpath: Vec<String>,
    env_vars: Vec<(String, String)>,
    interpreter: Interpreter,
    search_paths: Option<SearchPaths>,
    register_roots: bool,
    tag_specs: TagSpecDef,
}

impl ProjectFixture {
    #[must_use]
    pub(crate) fn new(root: impl Into<Utf8PathBuf>) -> Self {
        let settings = Settings::default();
        Self {
            root: root.into(),
            files: Vec::new(),
            django_settings_module: None,
            pythonpath: Vec::new(),
            env_vars: Vec::new(),
            interpreter: Interpreter::discover(settings.venv_path()),
            search_paths: None,
            register_roots: true,
            tag_specs: settings.tagspecs().clone(),
        }
    }

    #[must_use]
    pub(crate) fn file(mut self, path: impl Into<Utf8PathBuf>, source: impl Into<String>) -> Self {
        self.files.push((path.into(), source.into()));
        self
    }

    #[must_use]
    pub(crate) fn django_settings_module(mut self, module: impl Into<String>) -> Self {
        self.django_settings_module = Some(module.into());
        self
    }

    #[must_use]
    pub(crate) fn interpreter(mut self, interpreter: Interpreter) -> Self {
        self.interpreter = interpreter;
        self
    }

    #[must_use]
    pub(crate) fn search_paths(mut self, search_paths: SearchPaths) -> Self {
        self.search_paths = Some(search_paths);
        self
    }

    #[must_use]
    pub(crate) fn register_roots(mut self, register_roots: bool) -> Self {
        self.register_roots = register_roots;
        self
    }

    pub(crate) fn build(self, db: &TestDatabase) -> Project {
        for (path, source) in self.files {
            db.add_file(path.as_str(), &source);
        }

        let search_paths = self.search_paths.unwrap_or_else(|| {
            SearchPaths::from_project_settings(
                db.file_system(),
                &self.root,
                &self.interpreter,
                &self.pythonpath,
            )
        });
        if self.register_roots {
            search_paths.register_roots(db);
        }

        Project::new(
            db,
            self.root,
            search_paths,
            self.interpreter,
            self.django_settings_module,
            self.pythonpath,
            self.env_vars,
            self.tag_specs,
        )
    }

    pub(crate) fn install(self, db: &mut TestDatabase) -> Project {
        let project = self.build(db);
        db.set_project(project);
        project
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

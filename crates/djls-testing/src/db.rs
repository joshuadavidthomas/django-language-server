use std::sync::Arc;
use std::sync::Mutex;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_project::Db as ProjectDb;
use djls_project::ModelGraph;
use djls_project::Project;
use djls_project::TemplateLibraries;
use djls_semantic::Db as SemanticDb;
use djls_semantic::FilterAritySpecs;
use djls_semantic::TagSpecs;
use djls_semantic::builtin_tag_specs;
use djls_source::File;
use djls_source::FileSystem;
use djls_source::InMemoryFileSystem;
use djls_source::SourceFiles;

#[salsa::db]
#[derive(Clone)]
pub struct TestDatabase {
    fs: Arc<Mutex<InMemoryFileSystem>>,
    files: SourceFiles,
    tag_specs: TagSpecs,
    filter_arity_specs: FilterAritySpecs,
    template_libraries: TemplateLibraries,
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
        Self {
            fs: Arc::new(Mutex::new(InMemoryFileSystem::new())),
            files: SourceFiles::default(),
            tag_specs: builtin_tag_specs(),
            filter_arity_specs: FilterAritySpecs::new(),
            template_libraries: TemplateLibraries::default(),
            project: None,
            storage: salsa::Storage::default(),
        }
    }

    #[must_use]
    pub fn with_specs(mut self, specs: TagSpecs) -> Self {
        self.tag_specs = specs;
        self
    }

    #[must_use]
    pub fn with_arity_specs(mut self, specs: FilterAritySpecs) -> Self {
        self.filter_arity_specs = specs;
        self
    }

    #[must_use]
    pub fn with_template_libraries(mut self, template_libraries: TemplateLibraries) -> Self {
        self.template_libraries = template_libraries;
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

    #[must_use]
    pub fn get_or_create_file(&self, path: &Utf8Path) -> File {
        <Self as djls_source::Db>::get_or_create_file(self, path)
    }

    #[must_use]
    pub(crate) fn create_file_with_revision(&self, path: &Utf8Path, revision: u64) -> File {
        File::builder(path.to_owned(), revision)
            .durability(salsa::Durability::LOW)
            .path_durability(salsa::Durability::HIGH)
            .new(self)
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
impl SemanticDb for TestDatabase {
    fn tag_specs(&self) -> &TagSpecs {
        &self.tag_specs
    }

    fn template_dirs(&self) -> Option<Vec<Utf8PathBuf>> {
        self.project.and_then(|project| {
            let (dirs, knowledge) = djls_project::template_dirs(self, project);
            (*knowledge == djls_project::StaticKnowledge::Known).then(|| dirs.clone())
        })
    }

    fn diagnostics_config(&self) -> djls_conf::DiagnosticsConfig {
        djls_conf::DiagnosticsConfig::default()
    }

    fn template_libraries(&self) -> &TemplateLibraries {
        &self.template_libraries
    }

    fn filter_arity_specs(&self) -> &FilterAritySpecs {
        &self.filter_arity_specs
    }

    fn model_graph(&self) -> &ModelGraph {
        ModelGraph::empty_ref()
    }
}

use std::io;
use std::sync::Arc;
use std::sync::OnceLock;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_project::Project;
use djls_semantic::Db as SemanticDb;
use djls_semantic::FilterAritySpecs;
use djls_semantic::TagSpecs;
use djls_source::Db as SourceDb;
use djls_source::File;
use djls_source::FxDashMap;
use djls_source::SourceFiles;
use salsa::Setter;

#[salsa::db]
#[derive(Clone)]
pub struct Db {
    sources: Arc<FxDashMap<Utf8PathBuf, String>>,
    files: SourceFiles,
    tag_specs: Arc<TagSpecs>,
    template_libraries: Arc<djls_semantic::TemplateLibraries>,
    filter_arity_specs: Arc<FilterAritySpecs>,
    project: Arc<OnceLock<Project>>,
    storage: salsa::Storage<Self>,
}

impl Db {
    #[must_use]
    #[allow(clippy::missing_panics_doc)]
    pub fn new() -> Self {
        let db = Self {
            sources: Arc::new(FxDashMap::default()),
            files: SourceFiles::default(),
            tag_specs: Arc::new(TagSpecs::default()),
            template_libraries: Arc::new(djls_semantic::TemplateLibraries::default()),
            filter_arity_specs: Arc::new(FilterAritySpecs::new()),
            project: Arc::new(OnceLock::new()),
            storage: salsa::Storage::default(),
        };
        let project = Project::fixture_unavailable(&db);
        db.project
            .set(project)
            .expect("project should initialize once");
        db
    }

    #[must_use]
    pub fn with_tag_specs(mut self, specs: TagSpecs) -> Self {
        self.tag_specs = Arc::new(specs);
        self
    }

    #[must_use]
    pub fn with_template_libraries(mut self, libs: djls_semantic::TemplateLibraries) -> Self {
        self.template_libraries = Arc::new(libs);
        self
    }

    #[must_use]
    pub fn with_filter_arity_specs(mut self, specs: FilterAritySpecs) -> Self {
        self.filter_arity_specs = Arc::new(specs);
        self
    }

    pub fn file_with_contents(&mut self, path: impl Into<Utf8PathBuf>, contents: &str) -> File {
        let path = path.into();
        self.sources.insert(path.clone(), contents.to_string());
        self.files.get_or_create(self, &path)
    }

    pub fn set_file_contents(&mut self, file: File, contents: &str, revision: u64) {
        let path = file.path(self);
        self.sources.insert(path.clone(), contents.to_string());
        file.set_revision(self).to(revision);
    }
}

impl Default for Db {
    fn default() -> Self {
        Self::new()
    }
}

#[salsa::db]
impl salsa::Database for Db {}

#[salsa::db]
impl djls_project::Db for Db {
    fn project(&self) -> Project {
        *self.project.get().expect("project should be initialized")
    }
}

#[salsa::db]
impl SourceDb for Db {
    fn files(&self) -> &SourceFiles {
        &self.files
    }

    fn read_file(&self, path: &Utf8Path) -> io::Result<String> {
        Ok(self
            .sources
            .get(path)
            .map(|entry| entry.value().clone())
            .unwrap_or_default())
    }
}

#[salsa::db]
impl SemanticDb for Db {
    fn tag_specs(&self) -> &TagSpecs {
        &self.tag_specs
    }

    fn diagnostics_config(&self) -> djls_conf::DiagnosticsConfig {
        djls_conf::DiagnosticsConfig::default()
    }

    fn template_libraries(&self) -> &djls_semantic::TemplateLibraries {
        &self.template_libraries
    }

    fn filter_arity_specs(&self) -> &FilterAritySpecs {
        &self.filter_arity_specs
    }

    fn model_graph(&self) -> &djls_semantic::ModelGraph {
        djls_semantic::ModelGraph::empty_ref()
    }
}

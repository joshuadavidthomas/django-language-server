use std::io;
use std::sync::Arc;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_semantic::Db as SemanticDb;
use djls_semantic::FilterAritySpecs;
use djls_semantic::TagIndex;
use djls_semantic::TagSpecs;
use djls_source::Db as SourceDb;
use djls_source::File;
use djls_source::FxDashMap;
use djls_templates::Db as TemplateDb;
use salsa::Setter;

#[salsa::db]
#[derive(Clone)]
pub struct Db {
    sources: Arc<FxDashMap<Utf8PathBuf, String>>,
    tag_specs: Arc<TagSpecs>,
    template_libraries: Arc<djls_project::TemplateLibraries>,
    filter_arity_specs: Arc<FilterAritySpecs>,
    storage: salsa::Storage<Self>,
}

impl Db {
    #[must_use]
    pub fn new() -> Self {
        Self {
            sources: Arc::new(FxDashMap::default()),
            tag_specs: Arc::new(TagSpecs::default()),
            template_libraries: Arc::new(djls_project::TemplateLibraries::default()),
            filter_arity_specs: Arc::new(FilterAritySpecs::new()),
            storage: salsa::Storage::default(),
        }
    }

    #[must_use]
    pub fn with_tag_specs(mut self, specs: TagSpecs) -> Self {
        self.tag_specs = Arc::new(specs);
        self
    }

    #[must_use]
    pub fn with_template_libraries(mut self, libs: djls_project::TemplateLibraries) -> Self {
        self.template_libraries = Arc::new(libs);
        self
    }

    #[must_use]
    pub fn with_filter_arity_specs(mut self, specs: FilterAritySpecs) -> Self {
        self.filter_arity_specs = Arc::new(specs);
        self
    }

    pub fn file_with_contents(&mut self, path: Utf8PathBuf, contents: &str) -> File {
        self.sources.insert(path.clone(), contents.to_string());
        File::new(self, path, 0)
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
impl SourceDb for Db {
    fn create_file(&self, path: &Utf8Path) -> File {
        File::new(self, path.to_owned(), 0)
    }

    fn get_file(&self, _path: &Utf8Path) -> Option<File> {
        None
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
impl TemplateDb for Db {}

#[salsa::db]
impl SemanticDb for Db {
    fn tag_specs(&self) -> TagSpecs {
        (*self.tag_specs).clone()
    }

    fn tag_index(&self) -> TagIndex<'_> {
        TagIndex::from_specs(self)
    }

    fn template_dirs(&self) -> Option<Vec<Utf8PathBuf>> {
        None
    }

    fn diagnostics_config(&self) -> djls_conf::DiagnosticsConfig {
        djls_conf::DiagnosticsConfig::default()
    }

    fn template_libraries(&self) -> djls_project::TemplateLibraries {
        (*self.template_libraries).clone()
    }

    fn filter_arity_specs(&self) -> FilterAritySpecs {
        (*self.filter_arity_specs).clone()
    }
}

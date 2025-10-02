use std::io;
use std::sync::Arc;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_semantic::django_builtin_specs;
use djls_semantic::Db as SemanticDb;
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
    storage: salsa::Storage<Self>,
}

impl Db {
    #[must_use]
    pub fn new() -> Self {
        Self {
            sources: Arc::new(FxDashMap::default()),
            storage: salsa::Storage::default(),
        }
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
        django_builtin_specs()
    }

    fn tag_index(&self) -> TagIndex<'_> {
        TagIndex::from_specs(self)
    }

    fn template_dirs(&self) -> Option<Vec<Utf8PathBuf>> {
        None
    }
}

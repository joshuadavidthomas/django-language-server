use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::sync::Mutex;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_source::Db as SourceDb;
use djls_source::File;
use djls_templates::Db as TemplateDb;
use salsa::Setter;

#[salsa::db]
#[derive(Clone)]
pub struct Db {
    sources: Arc<Mutex<HashMap<Utf8PathBuf, String>>>,
    storage: salsa::Storage<Self>,
}

impl Db {
    #[must_use]
    pub fn new() -> Self {
        Self {
            sources: Arc::new(Mutex::new(HashMap::new())),
            storage: salsa::Storage::default(),
        }
    }

    /// ## Panics
    ///
    /// If sources mutex is poisoned.
    pub fn file_with_contents(&mut self, path: Utf8PathBuf, contents: &str) -> File {
        self.sources
            .lock()
            .expect("sources lock poisoned")
            .insert(path.clone(), contents.to_string());
        File::new(self, path, 0)
    }

    /// ## Panics
    ///
    /// If sources mutex is poisoned.
    pub fn set_file_contents(&mut self, file: File, contents: &str, revision: u64) {
        let path = file.path(self);
        self.sources
            .lock()
            .expect("sources lock poisoned")
            .insert(path.clone(), contents.to_string());
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
    fn read_file_source(&self, path: &Utf8Path) -> io::Result<String> {
        let sources = self.sources.lock().expect("sources lock poisoned");
        Ok(sources.get(path).cloned().unwrap_or_default())
    }
}

#[salsa::db]
impl TemplateDb for Db {}

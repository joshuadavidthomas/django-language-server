//! Browser entry point: an in-memory Django project analyzed with the production database.

use std::sync::Arc;
use std::sync::Mutex;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::Settings;
use djls_db::DjangoDatabase;
use djls_project::BundledFileSystem;
use djls_project::run_django_discovery;
use djls_source::File;
use djls_source::FileSystem;
use djls_source::InMemoryFileSystem;
use djls_source::PositionEncoding;
use djls_source::path_to_file;
use wasm_bindgen::prelude::*;

const PROJECT_ROOT: &str = "/project";

#[wasm_bindgen]
pub struct Workspace {
    files: Arc<Mutex<InMemoryFileSystem>>,
    db: DjangoDatabase,
}

#[wasm_bindgen]
impl Workspace {
    /// `settings` is the same JSON shape as `[tool.djls]`.
    #[wasm_bindgen(constructor)]
    pub fn new(settings: &str) -> Result<Workspace, JsError> {
        let settings: Settings = serde_json::from_str(settings)?;
        let files = Arc::new(Mutex::new(InMemoryFileSystem::new()));
        let file_system: Arc<dyn FileSystem> = Arc::new(BundledFileSystem::new(
            Arc::clone(&files) as Arc<dyn FileSystem>
        ));
        let mut db = DjangoDatabase::new(file_system, &settings, Some(Utf8Path::new(PROJECT_ROOT)));
        db.apply_project_settings(settings);
        Ok(Self { files, db })
    }

    /// Add or replace a project file. Paths are relative to the project root.
    #[wasm_bindgen(js_name = setFile)]
    pub fn set_file(&mut self, path: &str, content: String) {
        let path = project_path(path);
        self.files
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .add_file(path.clone(), content);
        File::sync_path(&mut self.db, &path);
    }

    #[wasm_bindgen(js_name = removeFile)]
    pub fn remove_file(&mut self, path: &str) {
        let path = project_path(path);
        self.files
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove_file(&path);
        File::sync_path(&mut self.db, &path);
    }

    /// Rerun Django Discovery; needed after settings or project-structure changes.
    pub fn discover(&mut self) -> Result<(), JsError> {
        run_django_discovery(&mut self.db)?;
        djls_ide::prepare_project_template_analysis(&self.db)
            .ok_or_else(|| JsError::new("Template analysis preparation was cancelled"))?;
        Ok(())
    }

    /// LSP `Diagnostic[]` as JSON, positioned in UTF-16 like browser editors expect.
    pub fn diagnostics(&self, path: &str) -> Result<String, JsError> {
        let file = path_to_file(&self.db, &project_path(path))?;
        let diagnostics = djls_ide::collect_diagnostics(&self.db, file, PositionEncoding::Utf16)
            .unwrap_or_default();
        Ok(serde_json::to_string(&diagnostics)?)
    }
}

fn project_path(path: &str) -> Utf8PathBuf {
    Utf8Path::new(PROJECT_ROOT).join(path.trim_start_matches('/'))
}

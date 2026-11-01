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
use djls_source::FileKind;
use djls_source::FileSystem;
use djls_source::InMemoryFileSystem;
use djls_source::LineCol;
use djls_source::Offset;
use djls_source::PositionEncoding;
use djls_source::path_to_file;
use tower_lsp_server::ls_types;
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
        // Embedded releases must be extracted before a database exists; see `registerDjango`.
        djls_project::prime_bundled_django_libraries();
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

    /// LSP `CompletionItem[]` as JSON; `line`/`character` are zero-based UTF-16.
    pub fn completions(&self, path: &str, line: u32, character: u32) -> Result<String, JsError> {
        let items = self
            .template_offset(path, line, character)?
            .and_then(|(file, offset)| {
                djls_ide::completion(&self.db, file, offset, PositionEncoding::Utf16, true)
            })
            .map(|response| match response {
                ls_types::CompletionResponse::Array(items) => items,
                ls_types::CompletionResponse::List(list) => list.items,
            })
            .unwrap_or_default();
        Ok(serde_json::to_string(&items)?)
    }

    /// LSP `Hover` or `null` as JSON.
    pub fn hover(&self, path: &str, line: u32, character: u32) -> Result<String, JsError> {
        let hover = self
            .template_offset(path, line, character)?
            .and_then(|(file, offset)| {
                djls_ide::hover(&self.db, file, offset, PositionEncoding::Utf16)
            });
        Ok(serde_json::to_string(&hover)?)
    }

    /// LSP `LocationLink[]` as JSON. Targets in bundled Django sources keep their
    /// virtual `file:///djls-cache/...` URI; read them back with `readFile`.
    pub fn definition(&self, path: &str, line: u32, character: u32) -> Result<String, JsError> {
        let links = self
            .template_offset(path, line, character)?
            .and_then(|(file, offset)| {
                djls_ide::goto_definition(&self.db, file, offset, true, PositionEncoding::Utf16)
            })
            .and_then(|response| match response {
                ls_types::GotoDefinitionResponse::Link(links) => Some(links),
                ls_types::GotoDefinitionResponse::Scalar(_)
                | ls_types::GotoDefinitionResponse::Array(_) => None,
            })
            .unwrap_or_default();
        Ok(serde_json::to_string(&links)?)
    }

    /// LSP `FoldingRange[]` as JSON.
    #[wasm_bindgen(js_name = foldingRanges)]
    pub fn folding_ranges(&self, path: &str) -> Result<String, JsError> {
        let file = path_to_file(&self.db, &project_path(path))?;
        let ranges = if is_template(&self.db, file) {
            djls_ide::collect_folding_ranges(&self.db, file)
        } else {
            Vec::new()
        };
        Ok(serde_json::to_string(&ranges)?)
    }

    /// LSP `TextEdit[]` as JSON. Like the server, this honors `format.enabled` (off by
    /// default) and `format.backend` from the settings passed to the constructor.
    pub fn format(
        &self,
        path: &str,
        #[wasm_bindgen(js_name = tabSize)] tab_size: u32,
        #[wasm_bindgen(js_name = insertSpaces)] insert_spaces: bool,
    ) -> Result<String, JsError> {
        let file = path_to_file(&self.db, &project_path(path))?;
        let format_config = self.db.settings().format().clone();
        let edits = if format_config.enabled() && is_template(&self.db, file) {
            djls_ide::format_document(
                &self.db,
                file,
                PositionEncoding::Utf16,
                format_config.backend(),
                &ls_types::FormattingOptions {
                    tab_size,
                    insert_spaces,
                    ..Default::default()
                },
            )
        } else {
            Vec::new()
        };
        Ok(serde_json::to_string(&edits)?)
    }

    /// Source text of a project file or a bundled file such as a definition target.
    /// Absolute paths are used as-is; relative paths are project-relative.
    #[wasm_bindgen(js_name = readFile)]
    pub fn read_file(&self, path: &str) -> Result<String, JsError> {
        let path = if path.starts_with('/') {
            Utf8PathBuf::from(path)
        } else {
            project_path(path)
        };
        let file = path_to_file(&self.db, &path)?;
        Ok(file.try_source(&self.db)?.as_str().to_string())
    }

    fn template_offset(
        &self,
        path: &str,
        line: u32,
        character: u32,
    ) -> Result<Option<(File, Offset)>, JsError> {
        let file = path_to_file(&self.db, &project_path(path))?;
        if !is_template(&self.db, file) {
            return Ok(None);
        }
        let source = file.try_source(&self.db)?;
        let offset = file.line_index(&self.db).offset(
            source.as_str(),
            LineCol::new(line, character),
            PositionEncoding::Utf16,
        );
        Ok(Some((file, offset)))
    }
}

fn is_template(db: &DjangoDatabase, file: File) -> bool {
    matches!(file.try_source(db), Ok(source) if *source.kind() == FileKind::Template)
}

fn project_path(path: &str) -> Utf8PathBuf {
    Utf8Path::new(PROJECT_ROOT).join(path.trim_start_matches('/'))
}

/// Supply a Django release this build does not embed: a Django wheel or a vendored bundle zip.
///
/// Returns its feature line, such as `"5.2"`. Register before any `Workspace` uses that release.
#[wasm_bindgen(js_name = registerDjango)]
pub fn register_django(archive: Vec<u8>) -> Result<String, JsError> {
    Ok(djls_project::register_django_bundle(archive)?
        .as_str()
        .to_owned())
}

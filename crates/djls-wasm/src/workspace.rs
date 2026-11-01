use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::Settings;
use djls_lsp::PositionExt;
use djls_source::File;
use djls_source::Offset;
use djls_source::PositionEncoding;
use tower_lsp_server::ls_types;
use wasm_bindgen::prelude::*;

use crate::project::Project;

const PROJECT_ROOT: &str = "/project";

/// A project the host drives file by file, as the playground does.
#[wasm_bindgen]
pub struct Workspace {
    project: Project,
}

#[wasm_bindgen]
impl Workspace {
    /// `settings` is the same JSON shape as `[tool.djls]`.
    #[wasm_bindgen(constructor)]
    pub fn new(settings: &str) -> Result<Workspace, JsError> {
        let settings: Settings = serde_json::from_str(settings)?;
        Ok(Self {
            project: Project::new(Some(Utf8Path::new(PROJECT_ROOT)), settings),
        })
    }

    /// Add or replace a project file. Paths are relative to the project root.
    #[wasm_bindgen(js_name = setFile)]
    pub fn set_file(&mut self, path: &str, content: String) {
        self.project.write_file(&project_path(path), content);
    }

    #[wasm_bindgen(js_name = removeFile)]
    pub fn remove_file(&mut self, path: &str) {
        self.project.remove_file(&project_path(path));
    }

    /// Rerun Django Discovery; needed after settings or project-structure changes.
    pub fn discover(&mut self) -> Result<(), JsError> {
        Ok(self.project.discover()?)
    }

    /// LSP `Diagnostic[]` as JSON, positioned in UTF-16 like browser editors expect.
    pub fn diagnostics(&self, path: &str) -> Result<String, JsError> {
        let file = self.file(path)?;
        let diagnostics =
            djls_ide::collect_diagnostics(self.project.db(), file, PositionEncoding::Utf16)
                .unwrap_or_default();
        Ok(serde_json::to_string(&diagnostics)?)
    }

    /// LSP `CompletionItem[]` as JSON; `line`/`character` are zero-based UTF-16.
    pub fn completions(&self, path: &str, line: u32, character: u32) -> Result<String, JsError> {
        let items = self
            .template_offset(path, line, character)?
            .and_then(|(file, offset)| {
                djls_ide::completion(
                    self.project.db(),
                    file,
                    offset,
                    PositionEncoding::Utf16,
                    true,
                )
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
                djls_ide::hover(self.project.db(), file, offset, PositionEncoding::Utf16)
            });
        Ok(serde_json::to_string(&hover)?)
    }

    /// LSP `LocationLink[]` as JSON. Targets in bundled Django sources keep their
    /// virtual `file:///djls-cache/...` URI; read them back with `readFile`.
    pub fn definition(&self, path: &str, line: u32, character: u32) -> Result<String, JsError> {
        let links = self
            .template_offset(path, line, character)?
            .and_then(|(file, offset)| {
                djls_ide::goto_definition(
                    self.project.db(),
                    file,
                    offset,
                    true,
                    PositionEncoding::Utf16,
                )
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
        let file = self.file(path)?;
        let ranges = if self.project.is_template(file) {
            djls_ide::collect_folding_ranges(self.project.db(), file)
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
        let edits = self.project.format(
            self.file(path)?,
            PositionEncoding::Utf16,
            &ls_types::FormattingOptions {
                tab_size,
                insert_spaces,
                ..Default::default()
            },
        );
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
        let file = djls_source::path_to_file(self.project.db(), &path)?;
        Ok(file.try_source(self.project.db())?.as_str().to_string())
    }

    fn file(&self, path: &str) -> Result<File, JsError> {
        Ok(djls_source::path_to_file(
            self.project.db(),
            &project_path(path),
        )?)
    }

    fn template_offset(
        &self,
        path: &str,
        line: u32,
        character: u32,
    ) -> Result<Option<(File, Offset)>, JsError> {
        let file = self.file(path)?;
        if !self.project.is_template(file) {
            return Ok(None);
        }
        let source = file.try_source(self.project.db())?;
        let offset = ls_types::Position::new(line, character).to_offset(
            source.as_str(),
            file.line_index(self.project.db()),
            PositionEncoding::Utf16,
        );
        Ok(Some((file, offset)))
    }
}

fn project_path(path: &str) -> Utf8PathBuf {
    Utf8Path::new(PROJECT_ROOT).join(path.trim_start_matches('/'))
}

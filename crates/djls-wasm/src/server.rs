//! A synchronous LSP server for browser hosts: one JSON-RPC message in, every outgoing
//! message it causes out. It follows `djls-server`'s request handling, minus threads, and
//! runs the Project work mutations require inline, before the next result that needs it.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_lsp::AnalysisHost;
use djls_lsp::InitializeParamsExt;
use djls_lsp::ProjectCoverage;
use djls_lsp::ProjectWork;
use djls_lsp::Workspace;
use djls_source::ChangeEvent;
use djls_source::FileSystem;
use djls_source::InMemoryFileSystem;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use serde_json::json;
use tower_lsp_server::ls_types;
use wasm_bindgen::prelude::*;

use crate::uri::UriMap;

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;
const SERVER_NOT_INITIALIZED: i64 = -32002;

/// An LSP server over an in-memory project. The host loads workspace files with the
/// `djls/syncFiles` and `djls/removeFiles` notifications; open documents override them.
#[wasm_bindgen]
pub struct LanguageServer {
    /// `None` until `initialize`, which supplies the root and client capabilities.
    host: Option<AnalysisHost>,
    /// The project as the host last synced it; open buffers take precedence.
    files: Arc<Mutex<InMemoryFileSystem>>,
    uris: UriMap,
    /// Coverage of the last successful priming, or `None` before one.
    coverage: Option<ProjectCoverage>,
    pending: Option<ProjectWork>,
    /// Diagnostics last pushed per open document, so unaffected documents stay quiet.
    published: HashMap<Utf8PathBuf, Vec<ls_types::Diagnostic>>,
    shut_down: bool,
}

impl Default for LanguageServer {
    fn default() -> Self {
        Self {
            host: None,
            files: Arc::new(Mutex::new(InMemoryFileSystem::new())),
            uris: UriMap::default(),
            coverage: None,
            pending: Some(ProjectWork::FullReload),
            published: HashMap::new(),
            shut_down: false,
        }
    }
}

#[wasm_bindgen]
impl LanguageServer {
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> LanguageServer {
        Self::default()
    }

    /// Handle one JSON-RPC message and return a JSON array of the messages to send back:
    /// the response when `message` is a request, then any notifications.
    #[must_use]
    pub fn handle(&mut self, message: &str) -> String {
        let mut outbox = Vec::new();
        match serde_json::from_str::<Incoming>(message) {
            Ok(Incoming {
                id: Some(id),
                method: Some(method),
                params,
            }) => {
                let response = match self.request(&method, params, &mut outbox) {
                    Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                    Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
                };
                outbox.insert(0, response);
            }
            Ok(Incoming {
                id: None,
                method: Some(method),
                params,
            }) => self.notification(&method, params, &mut outbox),
            // Responses to server requests; this server sends none that need answers.
            Ok(Incoming { method: None, .. }) => {}
            Err(error) => outbox.push(json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": ResponseError::new(PARSE_ERROR, error.to_string()),
            })),
        }
        Value::Array(outbox).to_string()
    }

    fn request(
        &mut self,
        method: &str,
        params: Value,
        outbox: &mut Vec<Value>,
    ) -> Result<Value, ResponseError> {
        if self.shut_down {
            return Err(ResponseError::new(
                INVALID_REQUEST,
                "server is shut down".to_string(),
            ));
        }
        if method == "initialize" {
            if self.host.is_some() {
                return Err(ResponseError::new(
                    INVALID_REQUEST,
                    "server is already initialized".to_string(),
                ));
            }
            return to_value(&self.initialize(&parse(params)?));
        }
        if self.host.is_none() {
            return Err(ResponseError::new(
                SERVER_NOT_INITIALIZED,
                "server is not initialized".to_string(),
            ));
        }
        match method {
            "shutdown" => {
                self.shut_down = true;
                Ok(Value::Null)
            }
            "textDocument/completion" => to_value(&self.completion(&parse(params)?, outbox)),
            "textDocument/hover" => to_value(&self.hover(&parse(params)?, outbox)),
            "textDocument/definition" => to_value(&self.definition(&parse(params)?, outbox)),
            "textDocument/foldingRange" => to_value(&self.folding_range(&parse(params)?, outbox)),
            "textDocument/formatting" => to_value(&self.formatting(&parse(params)?)),
            _ => Err(ResponseError::new(
                METHOD_NOT_FOUND,
                format!("unsupported request: {method}"),
            )),
        }
    }

    fn notification(&mut self, method: &str, params: Value, outbox: &mut Vec<Value>) {
        if self.host.is_none() || self.shut_down {
            return;
        }
        // Like the server, malformed notifications are dropped; there is no reply to carry an
        // error. `Some(edited)` means diagnostics may have changed, `edited` naming a document
        // whose new version must be published.
        let changed = match method {
            "textDocument/didOpen" => parse(params).map(|params| self.did_open(&params).map(Some)),
            "textDocument/didChange" => {
                parse(params).map(|params| self.did_change(params).map(Some))
            }
            "textDocument/didClose" => {
                parse(params).map(|params| self.did_close(&params).map(|()| None))
            }
            "workspace/didChangeConfiguration" => parse(params).map(|params| {
                self.did_change_configuration(params);
                Some(None)
            }),
            "djls/syncFiles" => parse(params).map(|params| {
                self.sync_files(params);
                Some(None)
            }),
            "djls/removeFiles" => parse(params).map(|params| {
                self.remove_files(&params);
                Some(None)
            }),
            // `initialized` needs nothing: Project work runs before the first result that needs it.
            _ => return,
        };
        if let Ok(Some(edited)) = changed {
            self.publish_diagnostics(edited.as_deref(), outbox);
        }
    }
}

impl LanguageServer {
    fn initialize(&mut self, params: &ls_types::InitializeParams) -> ls_types::InitializeResult {
        #[expect(
            deprecated,
            reason = "rootUri is the fallback for clients without folders"
        )]
        let root_uri = params
            .workspace_folders
            .as_ref()
            .and_then(|folders| folders.first())
            .map(|folder| &folder.uri)
            .or(params.root_uri.as_ref());
        let root = root_uri.and_then(|uri| self.uris.learn(uri));

        // Embedded releases must be extracted before a database exists; see `registerDjango`.
        djls_project::prime_bundled_django_libraries();
        let workspace = Workspace::with_disk(Arc::clone(&self.files) as Arc<dyn FileSystem>);
        let mut host = AnalysisHost::new(workspace, root.as_deref(), params);
        // There is no disk to load project configuration from: the client's settings are it.
        host.db_mut()
            .apply_project_settings(params.client_options().settings);

        // The server's capabilities for the features this server implements.
        let encoding = host.client_info().position_encoding();
        let all = djls_lsp::server_capabilities(encoding);
        let capabilities = ls_types::ServerCapabilities {
            position_encoding: all.position_encoding,
            text_document_sync: all.text_document_sync,
            completion_provider: all.completion_provider,
            hover_provider: all.hover_provider,
            definition_provider: all.definition_provider,
            folding_range_provider: all.folding_range_provider,
            document_formatting_provider: all.document_formatting_provider,
            ..Default::default()
        };
        self.host = Some(host);
        ls_types::InitializeResult {
            capabilities,
            server_info: Some(djls_lsp::server_info()),
            offset_encoding: Some(encoding.to_string()),
        }
    }

    fn did_open(&mut self, params: &ls_types::DidOpenTextDocumentParams) -> Option<Utf8PathBuf> {
        let path = self.uris.learn(&params.text_document.uri)?;
        let update =
            self.host
                .as_mut()?
                .open_document(&path, &params.text_document, self.coverage.as_ref());
        self.schedule(update.project_work);
        Some(path)
    }

    fn did_change(&mut self, params: ls_types::DidChangeTextDocumentParams) -> Option<Utf8PathBuf> {
        let path = self.uris.learn(&params.text_document.uri)?;
        let update = self.host.as_mut()?.update_document(
            &path,
            params.text_document.version,
            params.content_changes,
            self.coverage.as_ref(),
        )?;
        self.schedule(update.project_work);
        Some(path)
    }

    fn did_close(&mut self, params: &ls_types::DidCloseTextDocumentParams) -> Option<()> {
        let path = self.uris.learn(&params.text_document.uri)?;
        let update = self
            .host
            .as_mut()?
            .close_document(&path, self.coverage.as_ref())?;
        self.published.remove(&path);
        self.schedule(update.project_work);
        Some(())
    }

    /// Like the server, a configuration change reloads the project. Settings in the
    /// `initializationOptions` shape replace the current ones; anything else is ignored.
    fn did_change_configuration(&mut self, params: ls_types::DidChangeConfigurationParams) {
        let Some(host) = self.host.as_mut() else {
            return;
        };
        if params.settings.is_object() {
            let options = ls_types::InitializeParams {
                initialization_options: Some(params.settings),
                ..Default::default()
            }
            .client_options();
            if options.unknown.is_empty() {
                host.db_mut().apply_project_settings(options.settings);
            }
        }
        self.schedule(Some(ProjectWork::FullReload));
    }

    fn sync_files(&mut self, params: SyncFilesParams) {
        for file in params.files {
            let Some(path) = self.uris.learn(&file.uri) else {
                continue;
            };
            let change = {
                let mut files = self.files.lock().unwrap_or_else(PoisonError::into_inner);
                let existed = files.is_file(&path);
                files.add_file(path.clone(), file.text);
                if existed {
                    ChangeEvent::ContentChanged(path)
                } else {
                    ChangeEvent::BecameVisible(path)
                }
            };
            let Some(host) = self.host.as_mut() else {
                return;
            };
            let work = host.file_changed(&change, self.coverage.as_ref());
            self.schedule(work);
        }
    }

    fn remove_files(&mut self, params: &RemoveFilesParams) {
        for uri in &params.uris {
            let Some(path) = self.uris.learn(uri) else {
                continue;
            };
            let existed = {
                let mut files = self.files.lock().unwrap_or_else(PoisonError::into_inner);
                let existed = files.is_file(&path);
                files.remove_file(&path);
                existed
            };
            if existed && let Some(host) = self.host.as_mut() {
                let work = host.file_changed(&ChangeEvent::Deleted(path), self.coverage.as_ref());
                self.schedule(work);
            }
        }
    }

    fn schedule(&mut self, work: Option<ProjectWork>) {
        // A full reload subsumes a re-prime.
        self.pending = self.pending.max(work);
    }

    /// Run pending Project work; returns whether any ran.
    fn run_pending(&mut self, outbox: &mut Vec<Value>) -> bool {
        let Some(host) = self.host.as_mut() else {
            return false;
        };
        let Some(work) = self.pending.take() else {
            return false;
        };
        match host.run_project_work(work) {
            Ok(coverage) => self.coverage = coverage,
            Err(error) => {
                self.coverage = None;
                outbox.push(notification(
                    "window/logMessage",
                    &ls_types::LogMessageParams {
                        typ: ls_types::MessageType::ERROR,
                        message: format!("Django Discovery failed: {error}"),
                    },
                ));
            }
        }
        true
    }

    /// Push diagnostics for open documents: all of them after Project work ran, else
    /// `edited` and any whose diagnostics changed.
    fn publish_diagnostics(&mut self, edited: Option<&Utf8Path>, outbox: &mut Vec<Value>) {
        let reloaded = self.run_pending(outbox);
        let Some(host) = &self.host else {
            return;
        };
        let analysis = host.analysis();
        let encoding = analysis.client_info().position_encoding();
        for document in host.workspace().open_documents() {
            let path = document.path();
            let Some(file) = analysis.file(path) else {
                continue;
            };
            let Some(mut diagnostics) =
                djls_ide::collect_diagnostics(analysis.db(), file, encoding)
            else {
                continue;
            };
            for related in diagnostics
                .iter_mut()
                .flat_map(|diagnostic| diagnostic.related_information.iter_mut().flatten())
            {
                self.remap(&mut related.location.uri);
            }
            if !reloaded && edited != Some(path) && self.published.get(path) == Some(&diagnostics) {
                continue;
            }
            let Some(uri) = self.uris.uri(path) else {
                continue;
            };
            outbox.push(notification(
                "textDocument/publishDiagnostics",
                &ls_types::PublishDiagnosticsParams {
                    uri,
                    diagnostics: diagnostics.clone(),
                    version: Some(document.version()),
                },
            ));
            self.published.insert(path.to_path_buf(), diagnostics);
        }
    }

    fn completion(
        &mut self,
        params: &ls_types::CompletionParams,
        outbox: &mut Vec<Value>,
    ) -> Option<ls_types::CompletionResponse> {
        self.run_pending(outbox);
        let position = &params.text_document_position;
        let analysis = self.host.as_ref()?.analysis();
        let (file, offset) = analysis.template_position(
            &UriMap::path(&position.text_document.uri)?,
            position.position,
        )?;
        djls_ide::completion(
            analysis.db(),
            file,
            offset,
            analysis.client_info().position_encoding(),
            analysis.client_info().supports_snippets(),
        )
    }

    fn hover(
        &mut self,
        params: &ls_types::HoverParams,
        outbox: &mut Vec<Value>,
    ) -> Option<ls_types::Hover> {
        self.run_pending(outbox);
        let position = &params.text_document_position_params;
        let analysis = self.host.as_ref()?.analysis();
        let (file, offset) = analysis.template_position(
            &UriMap::path(&position.text_document.uri)?,
            position.position,
        )?;
        djls_ide::hover(
            analysis.db(),
            file,
            offset,
            analysis.client_info().position_encoding(),
        )
    }

    fn definition(
        &mut self,
        params: &ls_types::GotoDefinitionParams,
        outbox: &mut Vec<Value>,
    ) -> Option<ls_types::GotoDefinitionResponse> {
        self.run_pending(outbox);
        let position = &params.text_document_position_params;
        let analysis = self.host.as_ref()?.analysis();
        let (file, offset) = analysis.template_position(
            &UriMap::path(&position.text_document.uri)?,
            position.position,
        )?;
        let mut response = djls_ide::goto_definition(
            analysis.db(),
            file,
            offset,
            analysis.client_info().supports_location_links(),
            analysis.client_info().position_encoding(),
        )?;
        match &mut response {
            ls_types::GotoDefinitionResponse::Scalar(location) => {
                self.remap(&mut location.uri);
            }
            ls_types::GotoDefinitionResponse::Array(locations) => {
                for location in locations {
                    self.remap(&mut location.uri);
                }
            }
            ls_types::GotoDefinitionResponse::Link(links) => {
                for link in links {
                    self.remap(&mut link.target_uri);
                }
            }
        }
        Some(response)
    }

    fn folding_range(
        &mut self,
        params: &ls_types::FoldingRangeParams,
        outbox: &mut Vec<Value>,
    ) -> Vec<ls_types::FoldingRange> {
        self.run_pending(outbox);
        let Some(host) = &self.host else {
            return Vec::new();
        };
        let analysis = host.analysis();
        UriMap::path(&params.text_document.uri)
            .and_then(|path| analysis.template(&path))
            .map(|file| djls_ide::collect_folding_ranges(analysis.db(), file))
            .unwrap_or_default()
    }

    /// Syntax-only, like the server's: no Project work first.
    fn formatting(&self, params: &ls_types::DocumentFormattingParams) -> Vec<ls_types::TextEdit> {
        let Some(host) = &self.host else {
            return Vec::new();
        };
        UriMap::path(&params.text_document.uri)
            .map(|path| host.analysis().format(&path, &params.options))
            .unwrap_or_default()
    }

    /// `djls-ide` builds `file:` URIs; give the ones naming workspace files (synced or open)
    /// the workspace's scheme. Bundled Django sources keep theirs.
    fn remap(&self, uri: &mut ls_types::Uri) {
        if uri.scheme().as_str() != "file" {
            return;
        }
        if let Some(mapped) = UriMap::path(uri)
            .filter(|path| {
                self.host
                    .as_ref()
                    .is_some_and(|host| host.workspace().contains_document(path))
                    || self
                        .files
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .is_file(path)
            })
            .and_then(|path| self.uris.uri(&path))
        {
            *uri = mapped;
        }
    }
}

#[derive(Deserialize)]
struct SyncFilesParams {
    files: Vec<SyncedFile>,
}

#[derive(Deserialize)]
struct SyncedFile {
    uri: ls_types::Uri,
    text: String,
}

#[derive(Deserialize)]
struct RemoveFilesParams {
    uris: Vec<ls_types::Uri>,
}

#[derive(Deserialize)]
struct Incoming {
    id: Option<Value>,
    method: Option<String>,
    #[serde(default)]
    params: Value,
}

#[derive(serde::Serialize)]
struct ResponseError {
    code: i64,
    message: String,
}

impl ResponseError {
    fn new(code: i64, message: String) -> Self {
        Self { code, message }
    }
}

fn parse<T: DeserializeOwned>(params: Value) -> Result<T, ResponseError> {
    serde_json::from_value(params)
        .map_err(|error| ResponseError::new(INVALID_PARAMS, error.to_string()))
}

fn to_value(value: &impl serde::Serialize) -> Result<Value, ResponseError> {
    serde_json::to_value(value)
        .map_err(|error| ResponseError::new(INTERNAL_ERROR, error.to_string()))
}

fn notification(method: &str, params: &impl serde::Serialize) -> Value {
    json!({ "jsonrpc": "2.0", "method": method, "params": params })
}

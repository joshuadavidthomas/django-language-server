//! A synchronous LSP server for browser hosts: one JSON-RPC message in, every outgoing
//! message it causes out. It follows `djls-server`'s request handling, minus threads.

use std::collections::HashMap;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_lsp::ClientInfo;
use djls_lsp::InitializeParamsExt;
use djls_lsp::PositionExt;
use djls_lsp::TextDocumentContentChangeEventExt;
use djls_lsp::TextDocumentItemExt;
use djls_source::ChangeEvent;
use djls_source::File;
use djls_source::Offset;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use serde_json::json;
use tower_lsp_server::ls_types;
use wasm_bindgen::prelude::*;

use crate::project::Project;

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;
const SERVER_NOT_INITIALIZED: i64 = -32002;

/// An LSP server over an in-memory project. The host loads workspace files with the
/// `djls/syncFiles` and `djls/removeFiles` notifications; open documents override them.
#[wasm_bindgen]
#[derive(Default)]
pub struct LanguageServer {
    session: Option<Session>,
    shut_down: bool,
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
            if self.session.is_some() {
                return Err(ResponseError::new(
                    INVALID_REQUEST,
                    "server is already initialized".to_string(),
                ));
            }
            let (session, result) = Session::initialize(&parse(params)?);
            self.session = Some(session);
            return to_value(&result);
        }
        let Some(session) = &mut self.session else {
            return Err(ResponseError::new(
                SERVER_NOT_INITIALIZED,
                "server is not initialized".to_string(),
            ));
        };
        match method {
            "shutdown" => {
                self.shut_down = true;
                Ok(Value::Null)
            }
            "textDocument/completion" => to_value(&session.completion(&parse(params)?, outbox)),
            "textDocument/hover" => to_value(&session.hover(&parse(params)?, outbox)),
            "textDocument/definition" => to_value(&session.definition(&parse(params)?, outbox)),
            "textDocument/foldingRange" => {
                to_value(&session.folding_range(&parse(params)?, outbox))
            }
            "textDocument/formatting" => to_value(&session.formatting(&parse(params)?)),
            _ => Err(ResponseError::new(
                METHOD_NOT_FOUND,
                format!("unsupported request: {method}"),
            )),
        }
    }

    fn notification(&mut self, method: &str, params: Value, outbox: &mut Vec<Value>) {
        let Some(session) = self.session.as_mut().filter(|_| !self.shut_down) else {
            return;
        };
        // Like the server, malformed notifications are dropped; there is no reply to carry an
        // error. `Some(edited)` means diagnostics may have changed, `edited` naming a document
        // whose new version must be published.
        let changed = match method {
            "textDocument/didOpen" => {
                parse(params).map(|params| session.did_open(params).map(Some))
            }
            "textDocument/didChange" => {
                parse(params).map(|params| session.did_change(params).map(Some))
            }
            "textDocument/didClose" => {
                parse(params).map(|params| session.did_close(&params).map(|()| None))
            }
            "workspace/didChangeConfiguration" => parse(params).map(|params| {
                session.did_change_configuration(params);
                Some(None)
            }),
            "djls/syncFiles" => parse(params).map(|params| {
                session.sync_files(params);
                Some(None)
            }),
            "djls/removeFiles" => parse(params).map(|params| {
                session.remove_files(&params);
                Some(None)
            }),
            // `initialized` needs nothing: discovery runs before the first query that needs it.
            _ => return,
        };
        if let Ok(Some(edited)) = changed {
            session.publish_diagnostics(edited.as_deref(), outbox);
        }
    }
}

struct Session {
    project: Project,
    client_info: ClientInfo,
    uris: UriMap,
    needs_discovery: bool,
    /// Diagnostics last pushed per open document, so unaffected documents stay quiet.
    published: HashMap<Utf8PathBuf, Vec<ls_types::Diagnostic>>,
}

impl Session {
    fn initialize(params: &ls_types::InitializeParams) -> (Self, ls_types::InitializeResult) {
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
        let mut uris = UriMap::default();
        let root = root_uri.and_then(|uri| uris.learn(uri));

        let options = params.client_options();
        let settings = options.settings.clone();
        let client_info =
            ClientInfo::new(&params.capabilities, params.client_info.as_ref(), options);
        let encoding = client_info.position_encoding();

        // The server's capabilities for the features this server implements.
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
        let result = ls_types::InitializeResult {
            capabilities,
            server_info: Some(djls_lsp::server_info()),
            offset_encoding: Some(encoding.to_string()),
        };

        let session = Self {
            project: Project::new(root.as_deref(), settings),
            client_info,
            uris,
            needs_discovery: true,
            published: HashMap::new(),
        };
        (session, result)
    }

    fn did_open(&mut self, params: ls_types::DidOpenTextDocumentParams) -> Option<Utf8PathBuf> {
        let document = params.text_document;
        let path = self.uris.learn(&document.uri)?;
        let kind = document.language_id_to_file_kind(self.client_info.client());
        let change = self
            .project
            .workspace()
            .open_document_change(self.project.db(), &path);
        self.project
            .workspace_mut()
            .open_document(&path, &document.text, document.version, kind);
        self.apply(change);
        Some(path)
    }

    fn did_change(&mut self, params: ls_types::DidChangeTextDocumentParams) -> Option<Utf8PathBuf> {
        let path = self.uris.learn(&params.text_document.uri)?;
        let change = if self.project.workspace().contains_document(&path) {
            ChangeEvent::ContentChanged(path.clone())
        } else {
            self.project
                .workspace()
                .open_document_change(self.project.db(), &path)
        };
        self.project.workspace_mut().update_document(
            &path,
            params.content_changes.to_document_changes(),
            params.text_document.version,
            self.client_info.position_encoding(),
        )?;
        self.apply(change);
        Some(path)
    }

    fn did_close(&mut self, params: &ls_types::DidCloseTextDocumentParams) -> Option<()> {
        let path = self.uris.learn(&params.text_document.uri)?;
        let change = self.project.workspace().close_document_change(&path);
        self.project.workspace_mut().close_document(&path)?;
        self.published.remove(&path);
        self.apply(change);
        Some(())
    }

    /// Like the server, a configuration change reloads the project. Settings in the
    /// `initializationOptions` shape replace the current ones; anything else is ignored.
    fn did_change_configuration(&mut self, params: ls_types::DidChangeConfigurationParams) {
        if params.settings.is_object() {
            let options = ls_types::InitializeParams {
                initialization_options: Some(params.settings),
                ..Default::default()
            }
            .client_options();
            if options.unknown.is_empty() {
                self.project
                    .db_mut()
                    .apply_project_settings(options.settings);
            }
        }
        self.needs_discovery = true;
    }

    fn sync_files(&mut self, params: SyncFilesParams) {
        for file in params.files {
            let Some(path) = self.uris.learn(&file.uri) else {
                continue;
            };
            if let Some(change) = self.project.write_file(&path, file.text) {
                self.note(&change);
            }
        }
    }

    fn remove_files(&mut self, params: &RemoveFilesParams) {
        for uri in &params.uris {
            let Some(path) = self.uris.learn(uri) else {
                continue;
            };
            if let Some(change) = self.project.remove_file(&path) {
                self.note(&change);
            }
        }
    }

    fn apply(&mut self, change: ChangeEvent) {
        self.note(&change);
        self.project.apply(change);
    }

    fn note(&mut self, change: &ChangeEvent) {
        self.needs_discovery |= self.project.needs_discovery(change);
    }

    /// Run Django Discovery if a change since the last one needs it; returns whether it ran.
    fn ensure_discovered(&mut self, outbox: &mut Vec<Value>) -> bool {
        if !std::mem::take(&mut self.needs_discovery) {
            return false;
        }
        if let Err(error) = self.project.discover() {
            outbox.push(notification(
                "window/logMessage",
                &ls_types::LogMessageParams {
                    typ: ls_types::MessageType::ERROR,
                    message: format!("Django Discovery failed: {error}"),
                },
            ));
        }
        true
    }

    /// Push diagnostics for open documents: all of them after Django Discovery reran, else
    /// `edited` and any whose diagnostics changed.
    fn publish_diagnostics(&mut self, edited: Option<&Utf8Path>, outbox: &mut Vec<Value>) {
        let rediscovered = self.ensure_discovered(outbox);
        let encoding = self.client_info.position_encoding();
        for document in self.project.workspace().open_documents() {
            let path = document.path();
            let Some(file) = self.project.file(path) else {
                continue;
            };
            let Some(mut diagnostics) =
                djls_ide::collect_diagnostics(self.project.db(), file, encoding)
            else {
                continue;
            };
            for related in diagnostics
                .iter_mut()
                .flat_map(|diagnostic| diagnostic.related_information.iter_mut().flatten())
            {
                remap(&self.uris, &self.project, &mut related.location.uri);
            }
            if !rediscovered
                && edited != Some(path)
                && self.published.get(path) == Some(&diagnostics)
            {
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
        self.ensure_discovered(outbox);
        let (file, offset) = self.template_position(&params.text_document_position)?;
        djls_ide::completion(
            self.project.db(),
            file,
            offset,
            self.client_info.position_encoding(),
            self.client_info.supports_snippets(),
        )
    }

    fn hover(
        &mut self,
        params: &ls_types::HoverParams,
        outbox: &mut Vec<Value>,
    ) -> Option<ls_types::Hover> {
        self.ensure_discovered(outbox);
        let (file, offset) = self.template_position(&params.text_document_position_params)?;
        djls_ide::hover(
            self.project.db(),
            file,
            offset,
            self.client_info.position_encoding(),
        )
    }

    fn definition(
        &mut self,
        params: &ls_types::GotoDefinitionParams,
        outbox: &mut Vec<Value>,
    ) -> Option<ls_types::GotoDefinitionResponse> {
        self.ensure_discovered(outbox);
        let (file, offset) = self.template_position(&params.text_document_position_params)?;
        let mut response = djls_ide::goto_definition(
            self.project.db(),
            file,
            offset,
            self.client_info.supports_location_links(),
            self.client_info.position_encoding(),
        )?;
        match &mut response {
            ls_types::GotoDefinitionResponse::Scalar(location) => {
                remap(&self.uris, &self.project, &mut location.uri);
            }
            ls_types::GotoDefinitionResponse::Array(locations) => {
                for location in locations {
                    remap(&self.uris, &self.project, &mut location.uri);
                }
            }
            ls_types::GotoDefinitionResponse::Link(links) => {
                for link in links {
                    remap(&self.uris, &self.project, &mut link.target_uri);
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
        self.ensure_discovered(outbox);
        self.template(&params.text_document)
            .map(|file| djls_ide::collect_folding_ranges(self.project.db(), file))
            .unwrap_or_default()
    }

    /// Syntax-only, like the server's: no discovery first.
    fn formatting(&self, params: &ls_types::DocumentFormattingParams) -> Vec<ls_types::TextEdit> {
        UriMap::path(&params.text_document.uri)
            .and_then(|path| self.project.file(&path))
            .map(|file| {
                self.project
                    .format(file, self.client_info.position_encoding(), &params.options)
            })
            .unwrap_or_default()
    }

    fn template(&self, document: &ls_types::TextDocumentIdentifier) -> Option<File> {
        self.project.template(&UriMap::path(&document.uri)?)
    }

    fn template_position(
        &self,
        position: &ls_types::TextDocumentPositionParams,
    ) -> Option<(File, Offset)> {
        let file = self.template(&position.text_document)?;
        let source = file.try_source(self.project.db()).ok()?;
        let offset = position.position.to_offset(
            source.as_str(),
            file.line_index(self.project.db()),
            self.client_info.position_encoding(),
        );
        Some((file, offset))
    }
}

/// Maps document URIs to database paths and back. Browser workspaces use their folder's own
/// scheme (`vscode-vfs://github/...`, `vscode-test-web://mount/...`), so each other scheme and
/// authority gets a directory, `/{scheme}/{authority}`, and paths under it map back to that
/// URI prefix. `file:` URIs map to their path, which also covers bundled Django sources
/// under `/djls-cache`.
#[derive(Default)]
struct UriMap {
    /// Directory and URI prefix (`scheme://authority` or `scheme:`) of each scheme seen.
    mounts: Vec<(Utf8PathBuf, String)>,
}

impl UriMap {
    fn path(uri: &ls_types::Uri) -> Option<Utf8PathBuf> {
        Self::mount(uri).map(|(path, _, _)| path)
    }

    /// Like `path`, remembering the URI's scheme so paths can map back to it.
    fn learn(&mut self, uri: &ls_types::Uri) -> Option<Utf8PathBuf> {
        let (path, mount, prefix) = Self::mount(uri)?;
        if let Some(mount) = mount
            && !self.mounts.iter().any(|(known, _)| *known == mount)
        {
            self.mounts.push((mount, prefix));
        }
        Some(path)
    }

    fn mount(uri: &ls_types::Uri) -> Option<(Utf8PathBuf, Option<Utf8PathBuf>, String)> {
        let path = percent_encoding::percent_decode_str(uri.path().as_str())
            .decode_utf8()
            .ok()?;
        let scheme = uri.scheme().as_str();
        if scheme == "file" {
            return path
                .starts_with('/')
                .then(|| (Utf8PathBuf::from(path.as_ref()), None, String::new()));
        }
        if !(path.is_empty() || path.starts_with('/')) {
            return None;
        }
        let (mount, prefix) = match uri.authority() {
            Some(authority) => (
                Utf8PathBuf::from(format!("/{scheme}/{}", authority.as_str())),
                format!("{scheme}://{}", authority.as_str()),
            ),
            None => (
                Utf8PathBuf::from(format!("/{scheme}")),
                format!("{scheme}:"),
            ),
        };
        let path = mount.join(path.trim_start_matches('/'));
        Some((path, Some(mount), prefix))
    }

    fn uri(&self, path: &Utf8Path) -> Option<ls_types::Uri> {
        let uri = self
            .mounts
            .iter()
            .filter_map(|(mount, prefix)| Some((prefix, path.strip_prefix(mount).ok()?)))
            .min_by_key(|(_, rest)| rest.as_str().len())
            .map_or_else(
                || {
                    format!(
                        "file://{}",
                        percent_encoding::utf8_percent_encode(path.as_str(), PATH)
                    )
                },
                |(prefix, rest)| {
                    format!(
                        "{prefix}/{}",
                        percent_encoding::utf8_percent_encode(rest.as_str(), PATH)
                    )
                },
            );
        uri.parse().ok()
    }
}

/// `djls-ide` builds `file:` URIs; give the ones naming workspace files the workspace's
/// scheme. Bundled Django sources keep theirs.
fn remap(uris: &UriMap, project: &Project, uri: &mut ls_types::Uri) {
    if uri.scheme().as_str() != "file" {
        return;
    }
    if let Some(mapped) = UriMap::path(uri)
        .filter(|path| project.is_workspace_file(path))
        .and_then(|path| uris.uri(&path))
    {
        *uri = mapped;
    }
}

const PATH: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~')
    .remove(b'/');

#[derive(Deserialize)]
struct Incoming {
    id: Option<Value>,
    method: Option<String>,
    #[serde(default)]
    params: Value,
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

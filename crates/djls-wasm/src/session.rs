use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_lsp::InitializeParamsExt;
use djls_lsp::ProjectCoverage;
use djls_lsp::ProjectWork;
use djls_lsp::SessionCore;
use djls_lsp::Workspace;
use djls_source::ChangeEvent;
use djls_source::FileSystem;
use djls_source::InMemoryFileSystem;
use serde::Deserialize;
use serde_json::Value;
use tower_lsp_server::ls_types;

use crate::server::notification;
use crate::uri::UriMap;

/// The browser server's session: the shared [`SessionCore`] over files the host syncs in,
/// running the Project work mutations require inline, before the next result that needs it.
pub(crate) struct BrowserSession {
    core: SessionCore,
    /// The project as the host last synced it; open buffers take precedence.
    files: Arc<Mutex<InMemoryFileSystem>>,
    uris: UriMap,
    /// Coverage of the last successful priming, or `None` before one.
    coverage: Option<ProjectCoverage>,
    pending: Option<ProjectWork>,
    /// Diagnostics last pushed per open document, so unaffected documents stay quiet.
    published: HashMap<Utf8PathBuf, Vec<ls_types::Diagnostic>>,
}

impl BrowserSession {
    pub(crate) fn initialize(
        params: &ls_types::InitializeParams,
    ) -> (Self, ls_types::InitializeResult) {
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

        // Embedded releases must be extracted before a database exists; see `registerDjango`.
        djls_project::prime_bundled_django_libraries();
        let files = Arc::new(Mutex::new(InMemoryFileSystem::new()));
        let workspace = Workspace::with_disk(Arc::clone(&files) as Arc<dyn FileSystem>);
        let mut core = SessionCore::new(workspace, root.as_deref(), params);
        // There is no disk to load project configuration from: the client's settings are it.
        core.db_mut()
            .apply_project_settings(params.client_options().settings);

        // The server's capabilities for the features this server implements.
        let encoding = core.client_info().position_encoding();
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
            core,
            files,
            uris,
            coverage: None,
            pending: Some(ProjectWork::FullReload),
            published: HashMap::new(),
        };
        (session, result)
    }

    pub(crate) fn did_open(
        &mut self,
        params: &ls_types::DidOpenTextDocumentParams,
    ) -> Option<Utf8PathBuf> {
        let path = self.uris.learn(&params.text_document.uri)?;
        let update = self
            .core
            .open_document(&path, &params.text_document, self.coverage.as_ref());
        self.schedule(update.project_work);
        Some(path)
    }

    pub(crate) fn did_change(
        &mut self,
        params: ls_types::DidChangeTextDocumentParams,
    ) -> Option<Utf8PathBuf> {
        let path = self.uris.learn(&params.text_document.uri)?;
        let update = self.core.update_document(
            &path,
            params.text_document.version,
            params.content_changes,
            self.coverage.as_ref(),
        )?;
        self.schedule(update.project_work);
        Some(path)
    }

    pub(crate) fn did_close(
        &mut self,
        params: &ls_types::DidCloseTextDocumentParams,
    ) -> Option<()> {
        let path = self.uris.learn(&params.text_document.uri)?;
        let update = self.core.close_document(&path, self.coverage.as_ref())?;
        self.published.remove(&path);
        self.schedule(update.project_work);
        Some(())
    }

    /// Like the server, a configuration change reloads the project. Settings in the
    /// `initializationOptions` shape replace the current ones; anything else is ignored.
    pub(crate) fn did_change_configuration(
        &mut self,
        params: ls_types::DidChangeConfigurationParams,
    ) {
        if params.settings.is_object() {
            let options = ls_types::InitializeParams {
                initialization_options: Some(params.settings),
                ..Default::default()
            }
            .client_options();
            if options.unknown.is_empty() {
                self.core.db_mut().apply_project_settings(options.settings);
            }
        }
        self.schedule(Some(ProjectWork::FullReload));
    }

    pub(crate) fn sync_files(&mut self, params: SyncFilesParams) {
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
            let work = self.core.file_changed(&change, self.coverage.as_ref());
            self.schedule(work);
        }
    }

    pub(crate) fn remove_files(&mut self, params: &RemoveFilesParams) {
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
            if existed {
                let work = self
                    .core
                    .file_changed(&ChangeEvent::Deleted(path), self.coverage.as_ref());
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
        let Some(work) = self.pending.take() else {
            return false;
        };
        match self.core.run_project_work(work) {
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
    pub(crate) fn publish_diagnostics(
        &mut self,
        edited: Option<&Utf8Path>,
        outbox: &mut Vec<Value>,
    ) {
        let reloaded = self.run_pending(outbox);
        let analysis = self.core.analysis();
        let encoding = analysis.client_info().position_encoding();
        for document in self.core.workspace().open_documents() {
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

    pub(crate) fn completion(
        &mut self,
        params: &ls_types::CompletionParams,
        outbox: &mut Vec<Value>,
    ) -> Option<ls_types::CompletionResponse> {
        self.run_pending(outbox);
        let position = &params.text_document_position;
        let analysis = self.core.analysis();
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

    pub(crate) fn hover(
        &mut self,
        params: &ls_types::HoverParams,
        outbox: &mut Vec<Value>,
    ) -> Option<ls_types::Hover> {
        self.run_pending(outbox);
        let position = &params.text_document_position_params;
        let analysis = self.core.analysis();
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

    pub(crate) fn definition(
        &mut self,
        params: &ls_types::GotoDefinitionParams,
        outbox: &mut Vec<Value>,
    ) -> Option<ls_types::GotoDefinitionResponse> {
        self.run_pending(outbox);
        let position = &params.text_document_position_params;
        let analysis = self.core.analysis();
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

    pub(crate) fn folding_range(
        &mut self,
        params: &ls_types::FoldingRangeParams,
        outbox: &mut Vec<Value>,
    ) -> Vec<ls_types::FoldingRange> {
        self.run_pending(outbox);
        let analysis = self.core.analysis();
        UriMap::path(&params.text_document.uri)
            .and_then(|path| analysis.template(&path))
            .map(|file| djls_ide::collect_folding_ranges(analysis.db(), file))
            .unwrap_or_default()
    }

    /// Syntax-only, like the server's: no Project work first.
    pub(crate) fn formatting(
        &self,
        params: &ls_types::DocumentFormattingParams,
    ) -> Vec<ls_types::TextEdit> {
        UriMap::path(&params.text_document.uri)
            .map(|path| self.core.analysis().format(&path, &params.options))
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
                self.core.workspace().contains_document(path)
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
pub(crate) struct SyncFilesParams {
    files: Vec<SyncedFile>,
}

#[derive(Deserialize)]
struct SyncedFile {
    uri: ls_types::Uri,
    text: String,
}

#[derive(Deserialize)]
pub(crate) struct RemoveFilesParams {
    uris: Vec<ls_types::Uri>,
}

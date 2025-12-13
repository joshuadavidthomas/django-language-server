use std::future::Future;
use std::sync::Arc;

use djls_project::Db as ProjectDb;
use djls_semantic::Db as SemanticDb;
use djls_source::Db as SourceDb;
use djls_source::FileKind;
use djls_workspace::TextDocument;
use tokio::sync::Mutex;
use tower_lsp_server::jsonrpc::Result as LspResult;
use tower_lsp_server::ls_types;
use tower_lsp_server::Client;
use tower_lsp_server::LanguageServer;
use tracing_appender::non_blocking::WorkerGuard;

use crate::ext::PositionEncodingExt;
use crate::ext::PositionExt;
use crate::ext::TextDocumentIdentifierExt;
use crate::ext::UriExt;
use crate::queue::Queue;
use crate::session::Session;
use crate::session::SessionSnapshot;

pub struct DjangoLanguageServer {
    client: Client,
    session: Arc<Mutex<Session>>,
    queue: Queue,
    _log_guard: WorkerGuard,
}

impl DjangoLanguageServer {
    #[must_use]
    pub fn new(client: Client, log_guard: WorkerGuard) -> Self {
        Self {
            client,
            session: Arc::new(Mutex::new(Session::default())),
            queue: Queue::new(),
            _log_guard: log_guard,
        }
    }

    pub async fn with_session<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Session) -> R,
    {
        let session = self.session.lock().await;
        f(&session)
    }

    pub async fn with_session_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut Session) -> R,
    {
        let mut session = self.session.lock().await;
        f(&mut session)
    }

    pub async fn with_session_task<F, Fut>(&self, f: F)
    where
        F: FnOnce(SessionSnapshot) -> Fut + Send + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        let snapshot = {
            let session = self.session.lock().await;
            session.snapshot()
        };

        if let Err(e) = self.queue.submit(async move { f(snapshot).await }).await {
            tracing::error!("Failed to submit task: {}", e);
        } else {
            tracing::info!("Task submitted successfully");
        }
    }

    async fn publish_diagnostics(&self, document: &TextDocument) {
        let supports_pull = self
            .with_session(|session| session.client_info().supports_pull_diagnostics())
            .await;

        if supports_pull {
            tracing::debug!("Client supports pull diagnostics, skipping push");
            return;
        }

        let path = self
            .with_session(|session| document.path(session.db()).to_owned())
            .await;

        if FileKind::from(&path) != FileKind::Template {
            return;
        }

        let diagnostics: Vec<ls_types::Diagnostic> = self
            .with_session_mut(|session| {
                let db = session.db();
                let file = db.get_or_create_file(&path);
                let nodelist = djls_templates::parse_template(db, file);
                djls_ide::collect_diagnostics(db, file, nodelist)
            })
            .await;

        if let Some(lsp_uri) = ls_types::Uri::from_path(&path) {
            self.client
                .publish_diagnostics(lsp_uri, diagnostics.clone(), Some(document.version()))
                .await;

            tracing::debug!("Published {} diagnostics for {}", diagnostics.len(), path);
        }
    }
}

impl LanguageServer for DjangoLanguageServer {
    async fn initialize(
        &self,
        params: ls_types::InitializeParams,
    ) -> LspResult<ls_types::InitializeResult> {
        tracing::info!("Initializing server...");

        let session = Session::new(&params);
        let encoding = session.client_info().position_encoding();

        {
            let mut session_lock = self.session.lock().await;
            *session_lock = session;
        }

        Ok(ls_types::InitializeResult {
            capabilities: ls_types::ServerCapabilities {
                completion_provider: Some(ls_types::CompletionOptions {
                    resolve_provider: Some(false),
                    trigger_characters: Some(vec![
                        "{".to_string(),
                        "%".to_string(),
                        " ".to_string(),
                    ]),
                    ..Default::default()
                }),
                workspace: Some(ls_types::WorkspaceServerCapabilities {
                    workspace_folders: Some(ls_types::WorkspaceFoldersServerCapabilities {
                        supported: Some(true),
                        change_notifications: Some(ls_types::OneOf::Left(true)),
                    }),
                    file_operations: None,
                }),
                text_document_sync: Some(ls_types::TextDocumentSyncCapability::Options(
                    ls_types::TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(ls_types::TextDocumentSyncKind::INCREMENTAL),
                        will_save: Some(false),
                        will_save_wait_until: Some(false),
                        save: Some(ls_types::SaveOptions::default().into()),
                    },
                )),
                position_encoding: Some(encoding.to_lsp()),
                diagnostic_provider: Some(ls_types::DiagnosticServerCapabilities::Options(
                    ls_types::DiagnosticOptions {
                        identifier: None,
                        inter_file_dependencies: false,
                        workspace_diagnostics: false,
                        work_done_progress_options: ls_types::WorkDoneProgressOptions::default(),
                    },
                )),
                definition_provider: Some(ls_types::OneOf::Left(true)),
                references_provider: Some(ls_types::OneOf::Left(true)),
                ..Default::default()
            },
            server_info: Some(ls_types::ServerInfo {
                name: "Django Language Server".to_string(),
                version: Some(env!("DJLS_VERSION").to_string()),
            }),
            offset_encoding: Some(encoding.to_string()),
        })
    }

    async fn initialized(&self, _params: ls_types::InitializedParams) {
        tracing::info!("Server received initialized notification.");

        self.with_session_task(move |session| async move {
            if let Some(project) = session.db().project() {
                let path = project.root(session.db()).clone();
                tracing::info!("Task: Starting initialization for project at: {}", path);
                project.initialize(session.db());
                tracing::info!("Task: Successfully initialized project: {}", path);
            } else {
                tracing::info!("Task: No project configured, skipping initialization.");
            }

            Ok(())
        })
        .await;
    }

    async fn shutdown(&self) -> LspResult<()> {
        Ok(())
    }

    async fn did_open(&self, params: ls_types::DidOpenTextDocumentParams) {
        let document = self
            .with_session_mut(|session| session.open_document(&params.text_document))
            .await;

        if let Some(document) = document {
            self.publish_diagnostics(&document).await;
        }
    }

    async fn did_save(&self, params: ls_types::DidSaveTextDocumentParams) {
        let document = self
            .with_session_mut(|session| session.save_document(&params.text_document))
            .await;

        if let Some(document) = document {
            self.publish_diagnostics(&document).await;
        }
    }

    async fn did_change(&self, params: ls_types::DidChangeTextDocumentParams) {
        let document = self
            .with_session_mut(|session| {
                session.update_document(&params.text_document, params.content_changes)
            })
            .await;

        if let Some(document) = document {
            self.publish_diagnostics(&document).await;
        }
    }

    async fn did_close(&self, params: ls_types::DidCloseTextDocumentParams) {
        self.with_session_mut(|session| session.close_document(&params.text_document))
            .await;
    }

    async fn completion(
        &self,
        params: ls_types::CompletionParams,
    ) -> LspResult<Option<ls_types::CompletionResponse>> {
        let response = self
            .with_session_mut(|session| {
                let Some(path) = params
                    .text_document_position
                    .text_document
                    .uri
                    .to_utf8_path_buf()
                else {
                    tracing::debug!(
                        "Skipping non-file URI in completion: {}",
                        params.text_document_position.text_document.uri.as_str()
                    );
                    // TODO(virtual-paths): Support virtual documents with DocumentPath enum
                    return None;
                };

                tracing::debug!(
                    "Completion requested for {} at {:?}",
                    path,
                    params.text_document_position.position
                );

                let document = session.get_document(&path)?;
                let position = params.text_document_position.position;
                let encoding = session.client_info().position_encoding();
                let file_kind = FileKind::from(&path);
                let db = session.db();
                let template_tags = if let Some(project) = db.project() {
                    tracing::debug!("Fetching templatetags for project");
                    let tags = djls_project::templatetags(db, project);
                    if let Some(ref t) = tags {
                        tracing::debug!("Got {} templatetags", t.len());
                    } else {
                        tracing::warn!("No templatetags returned from project");
                    }
                    tags
                } else {
                    tracing::warn!("No project available for templatetags");
                    None
                };
                let tag_specs = db.tag_specs();
                let supports_snippets = session.client_info().supports_snippets();

                let completions = djls_ide::handle_completion(
                    &document,
                    position,
                    encoding,
                    file_kind,
                    template_tags.as_ref(),
                    Some(&tag_specs),
                    supports_snippets,
                );

                if completions.is_empty() {
                    None
                } else {
                    Some(ls_types::CompletionResponse::Array(completions))
                }
            })
            .await;

        Ok(response)
    }

    async fn diagnostic(
        &self,
        params: ls_types::DocumentDiagnosticParams,
    ) -> LspResult<ls_types::DocumentDiagnosticReportResult> {
        tracing::debug!(
            "Received diagnostic request for {:?}",
            params.text_document.uri
        );

        let diagnostics = if let Some(path) = params.text_document.uri.to_utf8_path_buf() {
            if FileKind::from(&path) == FileKind::Template {
                self.with_session_mut(move |session| {
                    let db = session.db_mut();
                    let file = db.get_or_create_file(&path);
                    let nodelist = djls_templates::parse_template(db, file);
                    djls_ide::collect_diagnostics(db, file, nodelist)
                })
                .await
            } else {
                vec![]
            }
        } else {
            tracing::debug!(
                "Skipping non-file URI in diagnostic: {}",
                params.text_document.uri.as_str()
            );
            // TODO(virtual-paths): Support virtual documents with DocumentPath enum
            vec![]
        };

        Ok(ls_types::DocumentDiagnosticReportResult::Report(
            ls_types::DocumentDiagnosticReport::Full(
                ls_types::RelatedFullDocumentDiagnosticReport {
                    related_documents: None,
                    full_document_diagnostic_report: ls_types::FullDocumentDiagnosticReport {
                        result_id: None,
                        items: diagnostics,
                    },
                },
            ),
        ))
    }

    async fn goto_definition(
        &self,
        params: ls_types::GotoDefinitionParams,
    ) -> LspResult<Option<ls_types::GotoDefinitionResponse>> {
        let response = self
            .with_session_mut(|session| {
                let encoding = session.client_info().position_encoding();
                let db = session.db_mut();
                let file = params
                    .text_document_position_params
                    .text_document
                    .to_file(db)?;
                let source = file.source(db);
                let line_index = file.line_index(db);
                let offset = params.text_document_position_params.position.to_offset(
                    source.as_str(),
                    line_index,
                    encoding,
                );
                djls_ide::goto_definition(db, file, offset)
            })
            .await;

        Ok(response)
    }

    async fn references(
        &self,
        params: ls_types::ReferenceParams,
    ) -> LspResult<Option<Vec<ls_types::Location>>> {
        let response = self
            .with_session_mut(|session| {
                let encoding = session.client_info().position_encoding();
                let db = session.db_mut();
                let file = params.text_document_position.text_document.to_file(db)?;
                let source = file.source(db);
                let line_index = file.line_index(db);
                let offset = params.text_document_position.position.to_offset(
                    source.as_str(),
                    line_index,
                    encoding,
                );
                djls_ide::find_references(db, file, offset)
            })
            .await;

        Ok(response)
    }

    async fn did_change_configuration(&self, _params: ls_types::DidChangeConfigurationParams) {
        tracing::info!("Configuration change detected. Reloading settings...");

        self.with_session_mut(|session| {
            if session.project().is_some() {
                let project_root = session.db().project_root_or_cwd();

                match djls_conf::Settings::new(
                    &project_root,
                    Some(session.client_info().config_overrides().clone()),
                ) {
                    Ok(new_settings) => {
                        session.set_settings(new_settings);
                    }
                    Err(e) => {
                        tracing::error!("Error loading settings: {}", e);
                    }
                }
            }
        })
        .await;
    }
}

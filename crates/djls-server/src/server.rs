use std::future::Future;
use std::sync::Arc;

use djls_project::Db as ProjectDb;
use djls_semantic::Db as SemanticDb;
use djls_source::Db as SourceDb;
use djls_source::FileKind;
use djls_workspace::TextDocument;
use tokio::sync::oneshot;
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

    pub async fn with_session_mut_task<F, Fut>(&self, f: F) -> oneshot::Receiver<anyhow::Result<()>>
    where
        F: FnOnce(Arc<Mutex<Session>>) -> Fut + Send + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        let session = Arc::clone(&self.session);
        let (tx, rx) = oneshot::channel();

        if let Err(e) = self
            .queue
            .submit(async move {
                let res = f(session).await;
                let _ = tx.send(res);
                Ok(())
            })
            .await
        {
            tracing::error!("Failed to submit task: {}", e);
        } else {
            tracing::info!("Task submitted successfully");
        }

        rx
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

    async fn republish_open_template_diagnostics(&self) {
        let documents = self.with_session(Session::open_documents).await;

        for document in documents {
            self.publish_diagnostics(&document).await;
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

        // Phase 1: Load cached inspector data for near-instant startup.
        // This populates template_libraries from disk cache so completions
        // and diagnostics work immediately while the real inspector runs.
        let cache_loaded = self
            .with_session_mut(|session| {
                let t = std::time::Instant::now();
                let loaded = session.db_mut().load_inspector_cache();
                if loaded {
                    tracing::info!("Inspector cache loaded in {:?}", t.elapsed());
                } else {
                    tracing::info!("No inspector cache available, will query inspector");
                }
                loaded
            })
            .await;

        // Phase 2: Run the real inspector query in the background.
        // This validates/refreshes the cached data, extracts external
        // rules, and initializes the workspace.
        let rx = self
            .with_session_mut_task(|session| async move {
                let start = std::time::Instant::now();

                let mut session_lock = session.lock().await;
                let db = session_lock.db_mut();

                let t = std::time::Instant::now();
                db.refresh_inspector();
                tracing::info!("Inspector refresh completed in {:?}", t.elapsed());

                if let Some(project) = db.project() {
                    let path = project.root(db).clone();
                    tracing::info!("Task: Starting initialization for project at: {}", path);
                    project.initialize(db);
                    tracing::info!("Task: Successfully initialized project: {}", path);
                } else {
                    tracing::info!("Task: No project configured, skipping initialization.");
                }

                tracing::info!("Server initialization completed in {:?}", start.elapsed());
                Ok(())
            })
            .await;

        // If we loaded from cache, the server is already functional — requests
        // arriving during the background refresh will use cached data. If no
        // cache was available, we wait for the full initialization like before.
        if !cache_loaded {
            let _ = rx.await;
        }
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
                let template_libraries = db.project().map(|project| project.template_libraries(db));

                let tag_specs = db.tag_specs();
                let supports_snippets = session.client_info().supports_snippets();

                // Compute position-aware available symbols for load-scoped completions.
                // Only computed when inspector-derived libraries are known and file is a template.
                let available_symbols = if file_kind == FileKind::Template {
                    if let Some(template_libraries) = template_libraries {
                        if template_libraries.inspector_knowledge == djls_project::Knowledge::Known
                        {
                            let file = db.get_or_create_file(&path);
                            let nodelist = djls_templates::parse_template(db, file);

                            nodelist.map(|nl| {
                                let loaded = djls_semantic::compute_loaded_libraries(db, nl);
                                let line_index = file.line_index(db);
                                let source_text = file.source(db);
                                let byte_offset = line_index.offset(
                                    source_text.as_str(),
                                    djls_source::LineCol::new(position.line, position.character),
                                    encoding,
                                );
                                djls_semantic::AvailableSymbols::at_position(
                                    &loaded,
                                    template_libraries,
                                    byte_offset.get(),
                                )
                            })
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };

                let completions = djls_ide::handle_completion(
                    &document,
                    position,
                    encoding,
                    file_kind,
                    template_libraries,
                    Some(&tag_specs),
                    available_symbols.as_ref(),
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

        let settings_update = self
            .with_session_mut(|session| {
                if session.project().is_none() {
                    return djls_db::SettingsUpdate::default();
                }

                let project_root = session.db().project_root_or_cwd();

                match djls_conf::Settings::new(
                    &project_root,
                    Some(session.client_info().config_overrides().clone()),
                ) {
                    Ok(new_settings) => session.set_settings(new_settings),
                    Err(e) => {
                        tracing::error!("Error loading settings: {}", e);
                        djls_db::SettingsUpdate::default()
                    }
                }
            })
            .await;

        if !settings_update.env_changed && !settings_update.diagnostics_changed {
            return;
        }

        if settings_update.env_changed {
            let rx = self
                .with_session_mut_task(|session| async move {
                    let start = std::time::Instant::now();

                    let mut session_lock = session.lock().await;
                    let db = session_lock.db_mut();

                    if db.project().is_none() {
                        return Ok(());
                    }

                    let t = std::time::Instant::now();
                    db.refresh_inspector();
                    tracing::info!("Inspector refresh completed in {:?}", t.elapsed());

                    if let Some(project) = db.project() {
                        project.initialize(db);
                    }

                    tracing::info!("Environment refresh completed in {:?}", start.elapsed());
                    Ok(())
                })
                .await;

            // Wait for environment update to complete before republishing diagnostics
            let _ = rx.await;
        }

        if settings_update.env_changed || settings_update.diagnostics_changed {
            self.republish_open_template_diagnostics().await;
        }
    }
}

use std::future::Future;
use std::sync::Arc;

use djls_semantic::ProjectDb;
use djls_semantic::load_template_library_cache;
use djls_semantic::refresh_external_data;
use djls_source::FileKind;
use tokio::sync::Mutex;
use tokio::sync::oneshot;
use tower_lsp_server::Client;
use tower_lsp_server::LanguageServer;
use tower_lsp_server::jsonrpc::Result as LspResult;
use tower_lsp_server::ls_types;

use crate::document::TextDocument;
use crate::ext::PositionEncodingExt;
use crate::ext::UriExt;
use crate::logging::LoggingGuard;
use crate::queue::Queue;
use crate::session::Session;

pub(crate) struct DjangoLanguageServer {
    client: Client,
    session: Arc<Mutex<Session>>,
    queue: Queue,
    logging: LoggingGuard,
}

impl DjangoLanguageServer {
    #[must_use]
    pub(crate) fn new(client: Client, logging: LoggingGuard) -> Self {
        Self {
            client,
            session: Arc::new(Mutex::new(Session::default())),
            queue: Queue::new(),
            logging,
        }
    }

    pub(crate) async fn with_session<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Session) -> R,
    {
        let session = self.session.lock().await;
        f(&session)
    }

    pub(crate) async fn with_session_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut Session) -> R,
    {
        let mut session = self.session.lock().await;
        f(&mut session)
    }

    pub(crate) async fn with_session_mut_task<F, Fut>(
        &self,
        f: F,
    ) -> oneshot::Receiver<anyhow::Result<()>>
    where
        F: FnOnce(Arc<Mutex<Session>>) -> Fut + Send + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        let session = Arc::clone(&self.session);
        let (tx, rx) = oneshot::channel();

        let submit_result = self
            .queue
            .submit(async move {
                let res = f(session).await;
                let _ = tx.send(res);
                Ok(())
            })
            .await;

        match submit_result {
            Ok(()) => {
                tracing::info!("Task submitted successfully");
            }
            Err(e) => {
                tracing::error!("Failed to submit task: {}", e);
            }
        }

        rx
    }

    async fn maybe_push_diagnostics(&self, document: &TextDocument) {
        let Some(diagnostics) = self
            .with_session(|session| {
                if session.client_info().supports_pull_diagnostics() {
                    tracing::debug!("Client supports pull diagnostics, skipping push");
                    return None;
                }

                djls_ide::collect_diagnostics(session.db(), document.file())
            })
            .await
        else {
            return;
        };

        let Some(lsp_uri) = ls_types::Uri::from_path(document.path()) else {
            return;
        };

        let diagnostic_count = diagnostics.len();
        let lsp_uri_text = lsp_uri.to_string();
        self.client
            .publish_diagnostics(lsp_uri, diagnostics, Some(document.version()))
            .await;

        tracing::debug!(
            "Published {} diagnostics for {}",
            diagnostic_count,
            lsp_uri_text
        );
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
                folding_range_provider: Some(ls_types::FoldingRangeProviderCapability::Simple(
                    true,
                )),
                document_symbol_provider: Some(ls_types::OneOf::Left(true)),
                hover_provider: Some(ls_types::HoverProviderCapability::Simple(true)),
                definition_provider: Some(ls_types::OneOf::Left(true)),
                references_provider: Some(ls_types::OneOf::Left(true)),
                document_formatting_provider: Some(ls_types::OneOf::Left(true)),
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

        // Phase 1: Load the cached template library snapshot for near-instant startup.
        // This populates template_libraries from disk cache so completions and
        // diagnostics work immediately while fresh project introspection runs.
        let cache_loaded = self
            .with_session_mut(|session| {
                let t = std::time::Instant::now();
                let loaded = load_template_library_cache(session.db_mut());
                if loaded {
                    tracing::info!(
                        "Template library snapshot cache loaded in {:?}",
                        t.elapsed()
                    );
                } else {
                    tracing::info!("No template library snapshot cache available");
                }
                loaded
            })
            .await;

        // Phase 2: Refresh project data in the background.
        // This validates/refreshes the cached data, extracts external
        // rules, and initializes the workspace.
        let rx = self
            .with_session_mut_task(|session| async move {
                let start = std::time::Instant::now();

                let mut session_lock = session.lock().await;
                let db = session_lock.db_mut();

                let t = std::time::Instant::now();
                refresh_external_data(db);
                tracing::info!("External data refresh completed in {:?}", t.elapsed());

                if db.project().is_none() {
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
        self.logging.disable_lsp();
        Ok(())
    }

    async fn did_open(&self, params: ls_types::DidOpenTextDocumentParams) {
        let document = self
            .with_session_mut(|session| session.open_document(&params.text_document))
            .await;

        if let Some(document) = document {
            self.maybe_push_diagnostics(&document).await;
        }
    }

    async fn did_save(&self, params: ls_types::DidSaveTextDocumentParams) {
        let document = self
            .with_session_mut(|session| session.save_document(&params.text_document))
            .await;

        if let Some(document) = document {
            self.maybe_push_diagnostics(&document).await;
        }
    }

    async fn did_change(&self, params: ls_types::DidChangeTextDocumentParams) {
        let document = self
            .with_session_mut(|session| {
                session.update_document(&params.text_document, params.content_changes)
            })
            .await;

        if let Some(document) = document {
            self.maybe_push_diagnostics(&document).await;
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
            .with_session(|session| {
                let (file, offset) = session.position_for_document_request(
                    &params.text_document_position.text_document,
                    params.text_document_position.position,
                    "completion",
                )?;
                let db = session.db();

                if *file.source(db).kind() != FileKind::Template {
                    return None;
                }

                djls_ide::completion(
                    db,
                    file,
                    offset,
                    session.client_info().position_encoding(),
                    session.client_info().supports_snippets(),
                )
            })
            .await;

        Ok(response)
    }

    async fn hover(&self, params: ls_types::HoverParams) -> LspResult<Option<ls_types::Hover>> {
        let response = self
            .with_session(|session| {
                let (file, offset) = session.position_for_document_request(
                    &params.text_document_position_params.text_document,
                    params.text_document_position_params.position,
                    "hover",
                )?;
                let db = session.db();

                if *file.source(db).kind() != FileKind::Template {
                    return None;
                }

                djls_ide::hover(db, file, offset)
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

        let diagnostics = self
            .with_session(|session| {
                let Some(file) =
                    session.file_for_document_request(&params.text_document, "diagnostic")
                else {
                    return Vec::new();
                };

                djls_ide::collect_diagnostics(session.db(), file).unwrap_or_default()
            })
            .await;

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

    async fn folding_range(
        &self,
        params: ls_types::FoldingRangeParams,
    ) -> LspResult<Option<Vec<ls_types::FoldingRange>>> {
        let ranges = self
            .with_session(|session| {
                let Some(file) =
                    session.file_for_document_request(&params.text_document, "folding")
                else {
                    return Vec::new();
                };
                let db = session.db();

                if *file.source(db).kind() != FileKind::Template {
                    return Vec::new();
                }

                djls_ide::collect_folding_ranges(db, file)
            })
            .await;

        Ok(Some(ranges))
    }

    async fn document_symbol(
        &self,
        params: ls_types::DocumentSymbolParams,
    ) -> LspResult<Option<ls_types::DocumentSymbolResponse>> {
        let symbols = self
            .with_session(|session| {
                let Some(file) =
                    session.file_for_document_request(&params.text_document, "document symbol")
                else {
                    return Vec::new();
                };
                let db = session.db();

                if *file.source(db).kind() != FileKind::Template {
                    return Vec::new();
                }

                djls_ide::document_symbols(db, file)
            })
            .await;

        Ok(Some(ls_types::DocumentSymbolResponse::Nested(symbols)))
    }

    async fn goto_definition(
        &self,
        params: ls_types::GotoDefinitionParams,
    ) -> LspResult<Option<ls_types::GotoDefinitionResponse>> {
        let response = self
            .with_session(|session| {
                let (file, offset) = session.position_for_document_request(
                    &params.text_document_position_params.text_document,
                    params.text_document_position_params.position,
                    "goto definition",
                )?;
                let db = session.db();

                if *file.source(db).kind() != FileKind::Template {
                    return None;
                }

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
            .with_session(|session| {
                let (file, offset) = session.position_for_document_request(
                    &params.text_document_position.text_document,
                    params.text_document_position.position,
                    "references",
                )?;
                let db = session.db();

                if *file.source(db).kind() != FileKind::Template {
                    return None;
                }

                djls_ide::find_references(db, file, offset)
            })
            .await;

        Ok(response)
    }

    async fn formatting(
        &self,
        params: ls_types::DocumentFormattingParams,
    ) -> LspResult<Option<Vec<ls_types::TextEdit>>> {
        let edits = self
            .with_session(|session| {
                let Some(file) =
                    session.file_for_document_request(&params.text_document, "formatting")
                else {
                    return Vec::new();
                };
                let db = session.db();
                let format_config = db.settings().format().clone();

                if !format_config.enabled() {
                    return Vec::new();
                }

                let source = file.source(db);
                if *source.kind() != FileKind::Template {
                    return Vec::new();
                }

                djls_ide::format_document(
                    db,
                    file,
                    session.client_info().position_encoding(),
                    format_config.backend(),
                    &params.options,
                )
            })
            .await;

        Ok(Some(edits))
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
                    refresh_external_data(db);
                    tracing::info!("External data refresh completed in {:?}", t.elapsed());

                    tracing::info!("Environment refresh completed in {:?}", start.elapsed());
                    Ok(())
                })
                .await;

            // Wait for environment update to complete before republishing diagnostics
            let _ = rx.await;
        }

        if settings_update.env_changed || settings_update.diagnostics_changed {
            let documents = self.with_session(Session::open_documents).await;

            for document in documents {
                self.maybe_push_diagnostics(&document).await;
            }
        }
    }
}

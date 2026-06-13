use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use djls_project::Db as ProjectDb;
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
use crate::refresh;
use crate::session::SNAPSHOT_CANCEL_RETRIES;
use crate::session::Session;
use crate::session::SessionSnapshot;

const PROJECT_REFRESH_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

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

    /// Capture a snapshot under a brief lock, then compute on the blocking
    /// pool so the single-threaded event loop stays responsive.
    pub(crate) async fn with_snapshot<F, R>(&self, f: F) -> R
    where
        F: Fn(&SessionSnapshot) -> R + Send + Sync + 'static,
        R: Default + Send + 'static,
    {
        let f = Arc::new(f);

        for attempt in 0..=SNAPSHOT_CANCEL_RETRIES {
            let snapshot = { self.session.lock().await.snapshot() };
            let f = Arc::clone(&f);
            let result = tokio::task::spawn_blocking(move || {
                salsa::Cancelled::catch(AssertUnwindSafe(|| f(&snapshot)))
            })
            .await
            .expect("snapshot task must not panic");

            match result {
                Ok(result) => return result,
                Err(cancelled) if attempt < SNAPSHOT_CANCEL_RETRIES => {
                    tracing::debug!(
                        ?cancelled,
                        attempt = attempt + 1,
                        "Snapshot request cancelled; retrying with fresh snapshot"
                    );
                }
                Err(cancelled) => {
                    tracing::debug!(
                        ?cancelled,
                        retries = SNAPSHOT_CANCEL_RETRIES,
                        "Snapshot request cancelled; returning fallback"
                    );
                    return R::default();
                }
            }
        }

        unreachable!("snapshot retry loop must return")
    }

    /// Bump the session's refresh epoch and queue a refresh task for the new
    /// epoch. The task body lives in [`crate::refresh`].
    async fn submit_project_refresh(&self, log_initialization: bool) {
        let client = self.client.clone();
        let (epoch, refresh_epoch, refresh_completion, diagnostic_publish_lock, client_info) = self
            .with_session(|session| {
                (
                    session.bump_refresh_epoch(),
                    session.refresh_epoch(),
                    session.refresh_completion(),
                    session.diagnostic_publish_lock(),
                    session.client_info().clone(),
                )
            })
            .await;

        let rx = self
            .with_session_mut_task(move |session| async move {
                let request = refresh::ProjectRefreshRequest::new(
                    refresh_epoch,
                    refresh_completion,
                    diagnostic_publish_lock,
                    client_info,
                    epoch,
                    log_initialization,
                );
                refresh::run_project_refresh(session, client, request).await
            })
            .await;
        drop(rx);
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

    async fn wait_for_current_project_refresh(&self, request: &str) {
        let Some((epoch, mut refresh_completion)) = self
            .with_session(|session| {
                let epoch = session.refresh_epoch_value();
                (session.db().project().is_some() && epoch > 0)
                    .then(|| (epoch, session.subscribe_refresh_completion()))
            })
            .await
        else {
            return;
        };

        let wait_for_completion = async {
            loop {
                if *refresh_completion.borrow_and_update() >= epoch {
                    return true;
                }

                if refresh_completion.changed().await.is_err() {
                    return false;
                }
            }
        };

        match tokio::time::timeout(PROJECT_REFRESH_REQUEST_TIMEOUT, wait_for_completion).await {
            Ok(true) => {}
            Ok(false) => {
                tracing::debug!(
                    request,
                    epoch,
                    "Refresh completion channel closed before project-aware request"
                );
            }
            Err(_) => {
                tracing::debug!(
                    request,
                    epoch,
                    timeout = ?PROJECT_REFRESH_REQUEST_TIMEOUT,
                    "Timed out waiting for project refresh before project-aware request"
                );
            }
        }
    }

    async fn maybe_push_diagnostics(&self, document: &TextDocument) {
        let file = document.file();
        let Some(diagnostics) = self
            .with_snapshot(move |snapshot| {
                if snapshot.client_info().supports_pull_diagnostics() {
                    tracing::debug!("Client supports pull diagnostics, skipping push");
                    return None;
                }

                djls_ide::collect_diagnostics(snapshot.db(), file)
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
        let publish_lock = self.with_session(Session::diagnostic_publish_lock).await;
        let _publish_guard = publish_lock.lock().await;
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

        // Refresh project data in the background and initialize the workspace.
        self.submit_project_refresh(true).await;
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
        self.wait_for_current_project_refresh("completion").await;

        let response = self
            .with_snapshot(move |snapshot| {
                let (file, offset) = snapshot.position_for_document_request(
                    &params.text_document_position.text_document,
                    params.text_document_position.position,
                    "completion",
                )?;
                let db = snapshot.db();

                if *file.source(db).kind() != FileKind::Template {
                    return None;
                }

                djls_ide::completion(
                    db,
                    file,
                    offset,
                    snapshot.client_info().position_encoding(),
                    snapshot.client_info().supports_snippets(),
                )
            })
            .await;

        Ok(response)
    }

    async fn hover(&self, params: ls_types::HoverParams) -> LspResult<Option<ls_types::Hover>> {
        self.wait_for_current_project_refresh("hover").await;

        let response = self
            .with_snapshot(move |snapshot| {
                let (file, offset) = snapshot.position_for_document_request(
                    &params.text_document_position_params.text_document,
                    params.text_document_position_params.position,
                    "hover",
                )?;
                let db = snapshot.db();

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
            .with_snapshot(move |snapshot| {
                let Some(file) =
                    snapshot.file_for_document_request(&params.text_document, "diagnostic")
                else {
                    return Vec::new();
                };

                djls_ide::collect_diagnostics(snapshot.db(), file).unwrap_or_default()
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
            .with_snapshot(move |snapshot| {
                let Some(file) =
                    snapshot.file_for_document_request(&params.text_document, "folding")
                else {
                    return Vec::new();
                };
                let db = snapshot.db();

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
            .with_snapshot(move |snapshot| {
                let Some(file) =
                    snapshot.file_for_document_request(&params.text_document, "document symbol")
                else {
                    return Vec::new();
                };
                let db = snapshot.db();

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
        self.wait_for_current_project_refresh("goto definition")
            .await;

        let response = self
            .with_snapshot(move |snapshot| {
                let (file, offset) = snapshot.position_for_document_request(
                    &params.text_document_position_params.text_document,
                    params.text_document_position_params.position,
                    "goto definition",
                )?;
                let db = snapshot.db();

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
        self.wait_for_current_project_refresh("references").await;

        let response = self
            .with_snapshot(move |snapshot| {
                let (file, offset) = snapshot.position_for_document_request(
                    &params.text_document_position.text_document,
                    params.text_document_position.position,
                    "references",
                )?;
                let db = snapshot.db();

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
            .with_snapshot(move |snapshot| {
                let Some(file) =
                    snapshot.file_for_document_request(&params.text_document, "formatting")
                else {
                    return Vec::new();
                };
                let db = snapshot.db();
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
                    snapshot.client_info().position_encoding(),
                    format_config.backend(),
                    &params.options,
                )
            })
            .await;

        Ok(Some(edits))
    }

    async fn did_change_configuration(&self, _params: ls_types::DidChangeConfigurationParams) {
        tracing::info!("Configuration change detected. Queuing project refresh...");
        self.submit_project_refresh(false).await;
    }
}

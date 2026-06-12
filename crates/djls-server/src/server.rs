use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use djls_project::Db as ProjectDb;
use djls_project::apply_refresh;
use djls_project::compute_refresh;
use djls_project::project_template_files;
use djls_semantic::Db as SemanticDb;
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
use crate::session::SessionSnapshot;

const SNAPSHOT_CANCEL_RETRIES: usize = 2;

pub(crate) struct DjangoLanguageServer {
    client: Client,
    session: Arc<Mutex<Session>>,
    queue: Queue,
    refresh_epoch: Arc<AtomicU64>,
    diagnostic_publish_lock: Arc<Mutex<()>>,
    logging: LoggingGuard,
}

impl DjangoLanguageServer {
    #[must_use]
    pub(crate) fn new(client: Client, logging: LoggingGuard) -> Self {
        Self {
            client,
            session: Arc::new(Mutex::new(Session::default())),
            queue: Queue::new(),
            refresh_epoch: Arc::new(AtomicU64::new(0)),
            diagnostic_publish_lock: Arc::new(Mutex::new(())),
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

    fn next_refresh_epoch(&self) -> u64 {
        self.refresh_epoch.fetch_add(1, Ordering::AcqRel) + 1
    }

    async fn submit_project_refresh(&self, epoch: u64, log_initialization: bool) {
        let client = self.client.clone();
        let refresh_epoch = Arc::clone(&self.refresh_epoch);
        let diagnostic_publish_lock = Arc::clone(&self.diagnostic_publish_lock);

        let rx = self
            .with_session_mut_task(move |session| async move {
                run_project_refresh_task(
                    session,
                    client,
                    refresh_epoch,
                    diagnostic_publish_lock,
                    epoch,
                    log_initialization,
                )
                .await
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
        let _publish_guard = self.diagnostic_publish_lock.lock().await;
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

fn refresh_is_stale(refresh_epoch: &AtomicU64, epoch: u64) -> bool {
    refresh_epoch.load(Ordering::Acquire) != epoch
}

async fn run_project_refresh_task(
    session: Arc<Mutex<Session>>,
    client: Client,
    refresh_epoch: Arc<AtomicU64>,
    diagnostic_publish_lock: Arc<Mutex<()>>,
    epoch: u64,
    log_initialization: bool,
) -> anyhow::Result<()> {
    let start = std::time::Instant::now();
    let result = run_project_refresh_task_inner(
        session,
        client,
        refresh_epoch,
        diagnostic_publish_lock,
        epoch,
    )
    .await;

    if log_initialization {
        tracing::info!("Server initialization completed in {:?}", start.elapsed());
    } else if result.is_ok() {
        tracing::info!("Environment refresh completed in {:?}", start.elapsed());
    }

    result
}

async fn run_project_refresh_task_inner(
    session: Arc<Mutex<Session>>,
    client: Client,
    refresh_epoch: Arc<AtomicU64>,
    diagnostic_publish_lock: Arc<Mutex<()>>,
    epoch: u64,
) -> anyhow::Result<()> {
    if refresh_is_stale(&refresh_epoch, epoch) {
        tracing::debug!(
            epoch,
            "Skipping stale project refresh before locking session"
        );
        return Ok(());
    }

    let Some(compute_db) = ({
        let session_lock = session.lock().await;
        if refresh_is_stale(&refresh_epoch, epoch) {
            tracing::debug!(
                epoch,
                "Skipping stale project refresh after locking session"
            );
            return Ok(());
        }

        let db = session_lock.db();
        db.project().map(|_| db.clone())
    }) else {
        tracing::info!("Task: No project configured, skipping initialization.");
        return Ok(());
    };

    let refresh = tokio::task::spawn_blocking(move || {
        salsa::Cancelled::catch(AssertUnwindSafe(|| compute_refresh(&compute_db)))
    })
    .await
    .expect("project refresh compute task must not panic");

    let Some(refresh) = (match refresh {
        Ok(refresh) => refresh,
        Err(cancelled) => {
            tracing::debug!(
                ?cancelled,
                "Project refresh compute cancelled; newer inputs will re-run refresh"
            );
            return Ok(());
        }
    }) else {
        return Ok(());
    };

    if refresh_is_stale(&refresh_epoch, epoch) {
        tracing::debug!(epoch, "Skipping stale project refresh before apply");
        return Ok(());
    }

    let (snapshot, documents) = {
        let mut session_lock = session.lock().await;
        if refresh_is_stale(&refresh_epoch, epoch) {
            tracing::debug!(epoch, "Skipping stale project refresh after apply lock");
            return Ok(());
        }

        let db = session_lock.db_mut();
        if db.project().is_none() {
            return Ok(());
        }

        let t = std::time::Instant::now();
        apply_refresh(db, refresh);
        tracing::info!("External data refresh completed in {:?}", t.elapsed());

        if refresh_is_stale(&refresh_epoch, epoch) {
            tracing::debug!(epoch, "Skipping stale project refresh after apply");
            return Ok(());
        }

        (session_lock.snapshot(), session_lock.open_documents())
    };

    warm_project_queries(snapshot.clone(), Arc::clone(&refresh_epoch), epoch).await;
    republish_snapshot_diagnostics(
        client,
        snapshot,
        documents,
        refresh_epoch,
        diagnostic_publish_lock,
        epoch,
    )
    .await;

    Ok(())
}

async fn warm_project_queries(
    snapshot: SessionSnapshot,
    refresh_epoch: Arc<AtomicU64>,
    epoch: u64,
) {
    let result = tokio::task::spawn_blocking(move || {
        salsa::Cancelled::catch(AssertUnwindSafe(|| {
            let db = snapshot.db();
            let Some(project) = db.project() else {
                return;
            };

            if refresh_is_stale(&refresh_epoch, epoch) {
                return;
            }
            let _ = db.tag_specs();

            if refresh_is_stale(&refresh_epoch, epoch) {
                return;
            }
            let _ = db.template_dirs();

            if refresh_is_stale(&refresh_epoch, epoch) {
                return;
            }
            let _ = db.template_libraries();

            if refresh_is_stale(&refresh_epoch, epoch) {
                return;
            }
            let _ = project_template_files(db, project);
        }))
    })
    .await
    .expect("project warm-up task must not panic");

    if let Err(cancelled) = result {
        tracing::debug!(
            ?cancelled,
            "Project refresh warm-up cancelled; newer inputs will re-warm queries"
        );
    }
}

async fn republish_snapshot_diagnostics(
    client: Client,
    snapshot: SessionSnapshot,
    documents: Vec<TextDocument>,
    refresh_epoch: Arc<AtomicU64>,
    diagnostic_publish_lock: Arc<Mutex<()>>,
    epoch: u64,
) {
    if snapshot.client_info().supports_pull_diagnostics() {
        tracing::debug!("Client supports pull diagnostics, skipping refresh diagnostics push");
        return;
    }

    for document in documents {
        if refresh_is_stale(&refresh_epoch, epoch) {
            tracing::debug!(epoch, "Skipping stale refresh diagnostics republish");
            return;
        }

        let file = document.file();
        let Some(diagnostics) = collect_snapshot_diagnostics(snapshot.clone(), file).await else {
            continue;
        };

        if refresh_is_stale(&refresh_epoch, epoch) {
            tracing::debug!(epoch, "Skipping stale refresh diagnostics publish");
            return;
        }

        let Some(lsp_uri) = ls_types::Uri::from_path(document.path()) else {
            continue;
        };

        let diagnostic_count = diagnostics.len();
        let lsp_uri_text = lsp_uri.to_string();
        let _publish_guard = diagnostic_publish_lock.lock().await;
        if refresh_is_stale(&refresh_epoch, epoch) {
            tracing::debug!(epoch, "Skipping stale refresh diagnostics publish");
            return;
        }
        client
            .publish_diagnostics(lsp_uri, diagnostics, Some(document.version()))
            .await;

        tracing::debug!(
            "Published {} diagnostics for {}",
            diagnostic_count,
            lsp_uri_text
        );
    }
}

async fn collect_snapshot_diagnostics(
    snapshot: SessionSnapshot,
    file: djls_source::File,
) -> Option<Vec<ls_types::Diagnostic>> {
    for attempt in 0..=SNAPSHOT_CANCEL_RETRIES {
        let snapshot = snapshot.clone();
        let result = tokio::task::spawn_blocking(move || {
            salsa::Cancelled::catch(AssertUnwindSafe(|| {
                djls_ide::collect_diagnostics(snapshot.db(), file)
            }))
        })
        .await
        .expect("diagnostics snapshot task must not panic");

        match result {
            Ok(diagnostics) => return diagnostics,
            Err(cancelled) if attempt < SNAPSHOT_CANCEL_RETRIES => {
                tracing::debug!(
                    ?cancelled,
                    attempt = attempt + 1,
                    "Refresh diagnostics cancelled; retrying with same snapshot"
                );
            }
            Err(cancelled) => {
                tracing::debug!(
                    ?cancelled,
                    retries = SNAPSHOT_CANCEL_RETRIES,
                    "Refresh diagnostics cancelled; skipping diagnostics republish"
                );
                return None;
            }
        }
    }

    unreachable!("diagnostics retry loop must return")
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
        let epoch = self.next_refresh_epoch();
        self.submit_project_refresh(epoch, true).await;
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

        if !settings_update.env_changed
            && !settings_update.diagnostics_changed
            && !settings_update.semantic_changed
        {
            return;
        }

        if settings_update.env_changed {
            let epoch = self.next_refresh_epoch();
            self.submit_project_refresh(epoch, false).await;
        } else if settings_update.diagnostics_changed || settings_update.semantic_changed {
            let _epoch = self.next_refresh_epoch();
            let documents = self.with_session(Session::open_documents).await;

            for document in documents {
                self.maybe_push_diagnostics(&document).await;
            }
        }
    }
}

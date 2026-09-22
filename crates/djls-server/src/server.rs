use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Instant;

use djls_ide::REPORT_UNREADABLE_REGISTRATION_COMMAND;
use djls_ide::ReportUnreadableRegistrationParams;
use djls_project::ScopedTemplateLibraries;
use djls_project::template_library_catalog;
use djls_project::template_library_definition_facts;
use djls_source::FileKind;
use djls_source::Span;
use percent_encoding::NON_ALPHANUMERIC;
use percent_encoding::utf8_percent_encode;
use salsa::Cancelled;
use tokio::sync::Mutex;
use tokio::task::spawn_blocking;
use tower_lsp_server::Client;
use tower_lsp_server::LanguageServer;
use tower_lsp_server::jsonrpc::Error as LspError;
use tower_lsp_server::jsonrpc::Result as LspResult;
use tower_lsp_server::ls_types;
use tracing::debug;
use tracing::error;

use crate::diagnostics::DiagnosticPublisher;
use crate::ext::PositionEncodingExt;
use crate::ext::UriExt;
use crate::logging::LspLogControl;
use crate::logging::blocking_in_span;
use crate::reload::ProjectReload;
use crate::session::CancellationRetryAction;
use crate::session::CancellationRetryState;
use crate::session::DocumentMutation;
use crate::session::IntrinsicReadinessState;
use crate::session::SNAPSHOT_CANCEL_RETRIES;
use crate::session::Session;
use crate::session::SessionSnapshot;

pub(crate) struct DjangoLanguageServer {
    client: Client,
    session: Arc<Mutex<Session>>,
    reload: ProjectReload,
    diagnostics: DiagnosticPublisher,
    logging: LspLogControl,
}

// Drop emits a terminal event even if the request future is dropped or
// panics, both reported as `cancelled`. This owns neither a snapshot nor an
// entered span guard, so it is safe across awaits.
struct RequestTimer {
    start: Instant,
    outcome: &'static str,
    operation: &'static str,
}

impl RequestTimer {
    fn new(operation: &'static str) -> Self {
        Self {
            start: Instant::now(),
            outcome: "cancelled",
            operation,
        }
    }

    fn finish(&mut self, outcome: &'static str) {
        self.outcome = outcome;
    }

    /// An empty result is normal; readiness or compute fallbacks that also
    /// return empty are distinguished by the nested snapshot's outcome.
    fn finish_result(&mut self, empty: bool) {
        self.finish(if empty { "empty" } else { "result" });
    }

    fn finish_mutation(&mut self, mutation: DocumentMutation) {
        self.finish(match mutation {
            DocumentMutation::Ignored => "ignored",
            DocumentMutation::Applied { .. } => "applied",
        });
    }
}

impl Drop for RequestTimer {
    fn drop(&mut self) {
        debug!(
            event = "request_completed",
            operation = self.operation,
            outcome = self.outcome,
            elapsed_ms = self.start.elapsed().as_secs_f64() * 1000.0
        );
    }
}

async fn traced_blocking<F, R>(f: F) -> Result<R, tokio::task::JoinError>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    spawn_blocking(blocking_in_span(tracing::Span::current(), f)).await
}

impl DjangoLanguageServer {
    #[must_use]
    pub(crate) fn new(client: Client, logging: LspLogControl) -> Self {
        let session = Arc::new(Mutex::new(Session::default()));
        let diagnostics = DiagnosticPublisher::new(Arc::clone(&session), client.clone());
        let reload = ProjectReload::new(Arc::clone(&session), client.clone(), diagnostics.clone());

        Self {
            client,
            session,
            reload,
            diagnostics,
            logging,
        }
    }

    #[tracing::instrument(level = "debug", skip_all, name = "session.mutation")]
    async fn with_session_mut<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&mut Session) -> R + Send + 'static,
        R: Send + 'static,
    {
        // Preserve lock acquisition order, but let the event loop release async-held
        // snapshots while a Salsa setter waits for their storage handles to drop.
        let mut session = Arc::clone(&self.session).lock_owned().await;
        if let Ok(result) = traced_blocking(move || f(&mut session)).await {
            Some(result)
        } else {
            error!(
                outcome = "blocking_task_failed",
                "Document mutation task failed"
            );
            None
        }
    }

    /// Wait for current-generation intrinsic products, atomically verify and
    /// capture that generation, then compute off the event loop.
    async fn with_ready_snapshot<F, R>(&self, f: F) -> R
    where
        F: Fn(&SessionSnapshot) -> R + Send + Sync + 'static,
        R: Default + Send + 'static,
    {
        with_ready_session_snapshot(&self.session, Arc::new(f)).await
    }

    /// Syntax-only requests may bypass project intrinsic readiness.
    async fn with_snapshot<F, R>(&self, f: F) -> R
    where
        F: Fn(&SessionSnapshot) -> R + Send + Sync + 'static,
        R: Default + Send + 'static,
    {
        with_session_snapshot(&self.session, Arc::new(f)).await
    }

    fn schedule_document_mutation(&self, mutation: DocumentMutation) {
        match mutation {
            DocumentMutation::Ignored => {}
            DocumentMutation::Applied { project_work, .. } => {
                if let Some(project_work) = project_work {
                    self.reload.request_current(project_work);
                }
                self.diagnostics.wake();
            }
        }
    }
}

#[tracing::instrument(level = "debug", skip_all, name = "session.syntax_snapshot")]
async fn with_session_snapshot<F, R>(session: &Arc<Mutex<Session>>, f: Arc<F>) -> R
where
    F: Fn(&SessionSnapshot) -> R + Send + Sync + 'static,
    R: Default + Send + 'static,
{
    let mut timer = RequestTimer::new("syntax_snapshot");
    let mut retry_state = CancellationRetryState::new();
    loop {
        let snapshot = { session.lock().await.snapshot() };
        let f = Arc::clone(&f);
        let Ok(result) =
            traced_blocking(move || Cancelled::catch(AssertUnwindSafe(|| f(&snapshot)))).await
        else {
            timer.finish("blocking_task_failed");
            error!("Syntax-only request snapshot task failed; returning fallback");
            return R::default();
        };
        match result {
            Ok(result) => {
                timer.finish("success");
                return result;
            }
            Err(_) => match retry_state.after_cancellation() {
                CancellationRetryAction::Retry { attempt } => {
                    debug!(attempt, "Syntax snapshot cancelled; retrying");
                }
                CancellationRetryAction::Exhausted => {
                    timer.finish("cancellation_exhausted");
                    debug!("Syntax snapshot cancelled; returning fallback");
                    return R::default();
                }
            },
        }
    }
}

#[tracing::instrument(level = "debug", skip_all, name = "session.ready_snapshot")]
async fn with_ready_session_snapshot<F, R>(session: &Arc<Mutex<Session>>, f: Arc<F>) -> R
where
    F: Fn(&SessionSnapshot) -> R + Send + Sync + 'static,
    R: Default + Send + 'static,
{
    let mut timer = RequestTimer::new("ready_snapshot");
    let mut retry_state = CancellationRetryState::new();
    loop {
        let Some(snapshot) = await_ready_session_snapshot(session).await else {
            timer.finish("readiness_failed");
            return R::default();
        };
        let f = Arc::clone(&f);
        let Ok(result) =
            traced_blocking(move || Cancelled::catch(AssertUnwindSafe(|| f(&snapshot)))).await
        else {
            timer.finish("blocking_task_failed");
            error!("Project-aware request snapshot task failed; returning fallback");
            return R::default();
        };

        match result {
            Ok(result) => {
                timer.finish("success");
                return result;
            }
            Err(_) => match retry_state.after_cancellation() {
                CancellationRetryAction::Retry { attempt } => {
                    debug!(
                        attempt,
                        "Snapshot request cancelled; retrying from intrinsic readiness"
                    );
                }
                CancellationRetryAction::Exhausted => {
                    timer.finish("cancellation_exhausted");
                    debug!(
                        retries = SNAPSHOT_CANCEL_RETRIES,
                        "Snapshot request cancelled; returning fallback"
                    );
                    return R::default();
                }
            },
        }
    }
}

async fn await_ready_session_snapshot(session: &Arc<Mutex<Session>>) -> Option<SessionSnapshot> {
    let start = Instant::now();
    let mut readiness = { session.lock().await.readiness_receiver() };
    loop {
        let observed = *readiness.borrow_and_update();
        match observed {
            IntrinsicReadinessState::Unready(_) => {
                if readiness.changed().await.is_err() {
                    debug!(
                        event = "readiness_completed",
                        outcome = "closed",
                        ready_wait_ms = start.elapsed().as_secs_f64() * 1000.0
                    );
                    return None;
                }
            }
            IntrinsicReadinessState::Failed(generation) => {
                let session = session.lock().await;
                if session.readiness_state() == IntrinsicReadinessState::Failed(generation) {
                    debug!(
                        event = "readiness_completed",
                        outcome = "failed",
                        generation,
                        ready_wait_ms = start.elapsed().as_secs_f64() * 1000.0
                    );
                    return None;
                }
            }
            IntrinsicReadinessState::ReadyWithoutProject | IntrinsicReadinessState::Ready(_) => {
                let session = session.lock().await;
                if session.readiness_state() != observed {
                    continue;
                }
                let snapshot = session.snapshot();
                debug_assert!(match observed {
                    IntrinsicReadinessState::ReadyWithoutProject => {
                        snapshot.intrinsic_generation().is_none()
                    }
                    IntrinsicReadinessState::Ready(generation) => {
                        snapshot.intrinsic_generation() == Some(generation)
                    }
                    IntrinsicReadinessState::Unready(_) | IntrinsicReadinessState::Failed(_) => {
                        false
                    }
                });
                debug!(
                    event = "readiness_completed",
                    outcome = "ready",
                    generation = snapshot.intrinsic_generation(),
                    ready_wait_ms = start.elapsed().as_secs_f64() * 1000.0
                );
                return Some(snapshot);
            }
        }
    }
}

const ISSUE_URL: &str = "https://github.com/joshuadavidthomas/django-language-server/issues/new";
const MAX_STATEMENT_LINES: usize = 40;
const MAX_ENCODED_ISSUE_BODY_BYTES: usize = 6_000;

fn parse_report_unreadable_registration_params(
    arguments: &[serde_json::Value],
) -> Result<ReportUnreadableRegistrationParams, String> {
    let [argument] = arguments else {
        return Err("reportUnreadableRegistration expects exactly one argument".to_string());
    };
    let params = ReportUnreadableRegistrationParams::from_lsp_value(argument)
        .map_err(|error| format!("invalid reportUnreadableRegistration argument: {error}"))?;
    if params.line == 0 {
        return Err("reportUnreadableRegistration line must be one-based".to_string());
    }
    if params.count == 0 {
        return Err("reportUnreadableRegistration count must be positive".to_string());
    }
    if params.shape.is_empty() {
        return Err("reportUnreadableRegistration shape must not be empty".to_string());
    }
    Ok(params)
}

fn statement_text(source: &str, span: Span) -> Option<String> {
    let statement = source.get(span.start_usize()..span.end_usize())?;
    Some(
        statement
            .lines()
            .take(MAX_STATEMENT_LINES)
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn report_unreadable_registration_issue_uri(
    db: &dyn djls_project::Db,
    params: &ReportUnreadableRegistrationParams,
) -> Option<ls_types::Uri> {
    let path = params.file.to_utf8_path_buf()?;
    let project = db.project()?;
    let library =
        ScopedTemplateLibraries::from_project_inventory(template_library_catalog(db, project))
            .resolved_libraries()
            .into_iter()
            .find(|library| {
                library.module_name_str() == params.module
                    && library
                        .source_file()
                        .is_some_and(|file| file.path(db).as_path() == path.as_path())
            })?;
    let file = library.source_file()?;
    let source = file.try_source(db).ok()?;
    if *source.kind() != FileKind::Python {
        return None;
    }

    let facts = template_library_definition_facts(db, library.id());
    let unread = facts.unread_registrations();
    if u32::try_from(unread.len()).ok()? != params.count {
        return None;
    }
    let registration = unread.first()?;
    let (line, _) = file
        .line_index(db)
        .to_line_col(registration.span.start_offset())
        .into();
    if line.saturating_add(1) != params.line || registration.shape.to_string() != params.shape {
        return None;
    }

    let statement = statement_text(source.as_str(), registration.span)?;
    unreadable_registration_issue_uri(params, &statement)
}

fn unreadable_registration_issue_body(
    params: &ReportUnreadableRegistrationParams,
    statement: Option<&str>,
) -> String {
    let statement_section = statement.map_or_else(
        || {
            format!(
                "- Statement (line {}): statement omitted, too long",
                params.line
            )
        },
        |statement| {
            format!(
                "- Statement (line {}):\n\n```python\n{}\n```",
                params.line, statement
            )
        },
    );
    format!(
        "DJLS could not read a registration in `{}`, so unrecognized tags and filters from that library are not reported.\n\n- DJLS version: {}\n- Shape: {}\n{}\n\n<!-- Check the snippet for anything private before submitting. -->",
        params.module,
        env!("DJLS_VERSION"),
        params.shape,
        statement_section,
    )
}

fn unreadable_registration_issue_uri(
    params: &ReportUnreadableRegistrationParams,
    statement: &str,
) -> Option<ls_types::Uri> {
    let title = format!("Unreadable tag registration: {}", params.shape);
    let mut body = unreadable_registration_issue_body(params, Some(statement));
    let encoded_body = utf8_percent_encode(&body, NON_ALPHANUMERIC).to_string();
    let encoded_body = if encoded_body.len() > MAX_ENCODED_ISSUE_BODY_BYTES {
        body = unreadable_registration_issue_body(params, None);
        utf8_percent_encode(&body, NON_ALPHANUMERIC).to_string()
    } else {
        encoded_body
    };
    if encoded_body.len() > MAX_ENCODED_ISSUE_BODY_BYTES {
        return None;
    }
    let encoded_title = utf8_percent_encode(&title, NON_ALPHANUMERIC);
    format!("{ISSUE_URL}?title={encoded_title}&body={encoded_body}")
        .parse()
        .ok()
}

impl LanguageServer for DjangoLanguageServer {
    #[tracing::instrument(
        level = "debug",
        skip_all,
        name = "lsp.request",
        fields(method = "initialize")
    )]
    async fn initialize(
        &self,
        params: ls_types::InitializeParams,
    ) -> LspResult<ls_types::InitializeResult> {
        let mut timer = RequestTimer::new("handler");
        tracing::info!("Initializing server...");

        let session = Session::new(&params);
        let encoding = session.client_info().position_encoding();

        {
            let mut session_lock = self.session.lock().await;
            *session_lock = session;
        }

        timer.finish("success");
        Ok(ls_types::InitializeResult {
            capabilities: ls_types::ServerCapabilities {
                completion_provider: Some(ls_types::CompletionOptions {
                    resolve_provider: Some(false),
                    trigger_characters: Some(vec![
                        "{".to_string(),
                        "%".to_string(),
                        " ".to_string(),
                        "\"".to_string(),
                        "'".to_string(),
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
                code_action_provider: Some(ls_types::CodeActionProviderCapability::Options(
                    ls_types::CodeActionOptions {
                        code_action_kinds: Some(vec![ls_types::CodeActionKind::QUICKFIX]),
                        work_done_progress_options: ls_types::WorkDoneProgressOptions::default(),
                        resolve_provider: Some(false),
                    },
                )),
                execute_command_provider: Some(ls_types::ExecuteCommandOptions {
                    commands: vec![REPORT_UNREADABLE_REGISTRATION_COMMAND.to_string()],
                    work_done_progress_options: ls_types::WorkDoneProgressOptions::default(),
                }),
                folding_range_provider: Some(ls_types::FoldingRangeProviderCapability::Simple(
                    true,
                )),
                document_symbol_provider: Some(ls_types::OneOf::Left(true)),
                document_link_provider: Some(ls_types::DocumentLinkOptions {
                    resolve_provider: Some(false),
                    work_done_progress_options: ls_types::WorkDoneProgressOptions::default(),
                }),
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

    #[tracing::instrument(
        level = "debug",
        skip_all,
        name = "lsp.notification",
        fields(method = "initialized")
    )]
    async fn initialized(&self, _params: ls_types::InitializedParams) {
        let mut timer = RequestTimer::new("handler");
        tracing::info!("Server received initialized notification.");

        self.reload.request_full_reload().await;
        timer.finish("scheduled");
    }

    #[tracing::instrument(
        level = "debug",
        skip_all,
        name = "lsp.request",
        fields(method = "shutdown")
    )]
    async fn shutdown(&self) -> LspResult<()> {
        let mut timer = RequestTimer::new("handler");
        timer.finish("success");
        self.logging.stop().await;
        Ok(())
    }

    #[tracing::instrument(
        level = "debug",
        skip_all,
        name = "lsp.notification",
        fields(method = "textDocument/didOpen")
    )]
    async fn did_open(&self, params: ls_types::DidOpenTextDocumentParams) {
        let mut timer = RequestTimer::new("handler");
        let Some(mutation) = self
            .with_session_mut(move |session| session.open_document(&params.text_document))
            .await
        else {
            timer.finish("blocking_task_failed");
            return;
        };

        self.schedule_document_mutation(mutation);
        timer.finish_mutation(mutation);
    }

    #[tracing::instrument(
        level = "debug",
        skip_all,
        name = "lsp.notification",
        fields(method = "textDocument/didSave")
    )]
    async fn did_save(&self, params: ls_types::DidSaveTextDocumentParams) {
        let mut timer = RequestTimer::new("handler");
        let Some(mutation) = self
            .with_session_mut(move |session| session.save_document(&params.text_document))
            .await
        else {
            timer.finish("blocking_task_failed");
            return;
        };

        self.schedule_document_mutation(mutation);
        timer.finish_mutation(mutation);
    }

    #[tracing::instrument(
        level = "debug",
        skip_all,
        name = "lsp.notification",
        fields(method = "textDocument/didChange")
    )]
    async fn did_change(&self, params: ls_types::DidChangeTextDocumentParams) {
        let mut timer = RequestTimer::new("handler");
        let Some(mutation) = self
            .with_session_mut(move |session| {
                session.update_document(&params.text_document, params.content_changes)
            })
            .await
        else {
            timer.finish("blocking_task_failed");
            return;
        };

        self.schedule_document_mutation(mutation);
        timer.finish_mutation(mutation);
    }

    #[tracing::instrument(
        level = "debug",
        skip_all,
        name = "lsp.notification",
        fields(method = "textDocument/didClose")
    )]
    async fn did_close(&self, params: ls_types::DidCloseTextDocumentParams) {
        let mut timer = RequestTimer::new("handler");
        let Some(mutation) = self
            .with_session_mut(move |session| session.close_document(&params.text_document))
            .await
        else {
            timer.finish("blocking_task_failed");
            return;
        };
        self.schedule_document_mutation(mutation);
        timer.finish_mutation(mutation);
    }

    #[tracing::instrument(
        level = "debug",
        skip_all,
        name = "lsp.request",
        fields(method = "textDocument/codeAction")
    )]
    async fn code_action(
        &self,
        params: ls_types::CodeActionParams,
    ) -> LspResult<Option<ls_types::CodeActionResponse>> {
        let mut timer = RequestTimer::new("handler");
        if params.context.only.as_ref().is_some_and(|only| {
            !only
                .iter()
                .any(|kind| kind == &ls_types::CodeActionKind::QUICKFIX)
        }) {
            timer.finish("empty");
            return Ok(None);
        }

        let response = self
            .with_ready_snapshot(move |snapshot| {
                let (file, range) = snapshot.range_for_document_request(
                    &params.text_document,
                    params.range,
                )?;
                let db = snapshot.db();

                if !matches!(file.try_source(db), Ok(source) if *source.kind() == FileKind::Template)
                {
                    return None;
                }

                djls_ide::code_actions(db, file, range, snapshot.client_info().position_encoding())
            })
            .await;

        timer.finish_result(response.as_ref().is_none_or(Vec::is_empty));
        Ok(response)
    }

    #[tracing::instrument(
        level = "debug",
        skip_all,
        name = "lsp.request",
        fields(method = "workspace/executeCommand")
    )]
    async fn execute_command(
        &self,
        params: ls_types::ExecuteCommandParams,
    ) -> LspResult<Option<serde_json::Value>> {
        let mut timer = RequestTimer::new("handler");
        if params.command != REPORT_UNREADABLE_REGISTRATION_COMMAND {
            timer.finish("invalid_params");
            return Err(LspError::invalid_params(format!(
                "unknown command: {}",
                params.command
            )));
        }
        let report =
            parse_report_unreadable_registration_params(&params.arguments).map_err(|error| {
                timer.finish("invalid_params");
                LspError::invalid_params(error)
            })?;
        let issue_uri = self
            .with_ready_snapshot(move |snapshot| {
                report_unreadable_registration_issue_uri(snapshot.db(), &report)
            })
            .await
            .ok_or_else(|| {
                timer.finish("invalid_params");
                LspError::invalid_params(
                    "reportUnreadableRegistration arguments do not match an unread registration",
                )
            })?;

        self.client
            .show_document(ls_types::ShowDocumentParams {
                uri: issue_uri,
                external: Some(true),
                take_focus: None,
                selection: None,
            })
            .await
            .inspect_err(|_error| {
                timer.finish("client_error");
            })?;

        timer.finish("success");
        Ok(None)
    }

    #[tracing::instrument(
        level = "debug",
        skip_all,
        name = "lsp.request",
        fields(method = "textDocument/completion")
    )]
    async fn completion(
        &self,
        params: ls_types::CompletionParams,
    ) -> LspResult<Option<ls_types::CompletionResponse>> {
        let mut timer = RequestTimer::new("handler");
        let response = self
            .with_ready_snapshot(move |snapshot| {
                let (file, offset) = snapshot.position_for_document_request(
                    &params.text_document_position.text_document,
                    params.text_document_position.position,
                )?;
                let db = snapshot.db();

                if !matches!(file.try_source(db), Ok(source) if *source.kind() == FileKind::Template)
                {
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

        let result_count = match &response {
            None => 0,
            Some(ls_types::CompletionResponse::Array(items)) => items.len(),
            Some(ls_types::CompletionResponse::List(list)) => list.items.len(),
        };
        debug!(result_count);
        timer.finish_result(result_count == 0);
        Ok(response)
    }

    #[tracing::instrument(
        level = "debug",
        skip_all,
        name = "lsp.request",
        fields(method = "textDocument/hover")
    )]
    async fn hover(&self, params: ls_types::HoverParams) -> LspResult<Option<ls_types::Hover>> {
        let mut timer = RequestTimer::new("handler");
        let response = self
            .with_ready_snapshot(move |snapshot| {
                let (file, offset) = snapshot.position_for_document_request(
                    &params.text_document_position_params.text_document,
                    params.text_document_position_params.position,
                )?;
                let db = snapshot.db();

                if !matches!(file.try_source(db), Ok(source) if *source.kind() == FileKind::Template)
                {
                    return None;
                }

                djls_ide::hover(
                    db,
                    file,
                    offset,
                    snapshot.client_info().position_encoding(),
                )
            })
            .await;

        timer.finish_result(response.is_none());
        Ok(response)
    }

    #[tracing::instrument(
        level = "debug",
        skip_all,
        name = "lsp.request",
        fields(method = "textDocument/diagnostic")
    )]
    async fn diagnostic(
        &self,
        params: ls_types::DocumentDiagnosticParams,
    ) -> LspResult<ls_types::DocumentDiagnosticReportResult> {
        let mut timer = RequestTimer::new("handler");

        let diagnostics = self
            .with_ready_snapshot(move |snapshot| {
                let Some(file) = snapshot.file_for_document_request(&params.text_document) else {
                    return Vec::new();
                };

                djls_ide::collect_diagnostics(
                    snapshot.db(),
                    file,
                    snapshot.client_info().position_encoding(),
                )
                .unwrap_or_default()
            })
            .await;

        debug!(result_count = diagnostics.len());
        timer.finish_result(diagnostics.is_empty());
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

    #[tracing::instrument(
        level = "debug",
        skip_all,
        name = "lsp.request",
        fields(method = "textDocument/foldingRange")
    )]
    async fn folding_range(
        &self,
        params: ls_types::FoldingRangeParams,
    ) -> LspResult<Option<Vec<ls_types::FoldingRange>>> {
        let mut timer = RequestTimer::new("handler");
        let ranges = self
            .with_ready_snapshot(move |snapshot| {
                let Some(file) =
                    snapshot.file_for_document_request(&params.text_document)
                else {
                    return Vec::new();
                };
                let db = snapshot.db();

                if !matches!(file.try_source(db), Ok(source) if *source.kind() == FileKind::Template)
                {
                    return Vec::new();
                }

                djls_ide::collect_folding_ranges(db, file)
            })
            .await;

        debug!(result_count = ranges.len());
        timer.finish_result(ranges.is_empty());
        Ok(Some(ranges))
    }

    #[tracing::instrument(
        level = "debug",
        skip_all,
        name = "lsp.request",
        fields(method = "textDocument/documentSymbol")
    )]
    async fn document_symbol(
        &self,
        params: ls_types::DocumentSymbolParams,
    ) -> LspResult<Option<ls_types::DocumentSymbolResponse>> {
        let mut timer = RequestTimer::new("handler");
        let symbols = self
            .with_ready_snapshot(move |snapshot| {
                let Some(file) =
                    snapshot.file_for_document_request(&params.text_document)
                else {
                    return Vec::new();
                };
                let db = snapshot.db();

                if !matches!(file.try_source(db), Ok(source) if *source.kind() == FileKind::Template)
                {
                    return Vec::new();
                }

                djls_ide::document_symbols(
                    db,
                    file,
                    snapshot.client_info().position_encoding(),
                )
            })
            .await;

        debug!(result_count = symbols.len());
        timer.finish_result(symbols.is_empty());
        Ok(Some(ls_types::DocumentSymbolResponse::Nested(symbols)))
    }

    #[tracing::instrument(
        level = "debug",
        skip_all,
        name = "lsp.request",
        fields(method = "textDocument/documentLink")
    )]
    async fn document_link(
        &self,
        params: ls_types::DocumentLinkParams,
    ) -> LspResult<Option<Vec<ls_types::DocumentLink>>> {
        let mut timer = RequestTimer::new("handler");
        let links = self
            .with_ready_snapshot(move |snapshot| {
                let Some(file) =
                    snapshot.file_for_document_request(&params.text_document)
                else {
                    return Vec::new();
                };
                let db = snapshot.db();

                if !matches!(file.try_source(db), Ok(source) if *source.kind() == FileKind::Template)
                {
                    return Vec::new();
                }

                djls_ide::document_links(
                    db,
                    file,
                    snapshot.client_info().position_encoding(),
                )
            })
            .await;

        debug!(result_count = links.len());
        timer.finish_result(links.is_empty());
        Ok(Some(links))
    }

    #[tracing::instrument(
        level = "debug",
        skip_all,
        name = "lsp.request",
        fields(method = "textDocument/definition")
    )]
    async fn goto_definition(
        &self,
        params: ls_types::GotoDefinitionParams,
    ) -> LspResult<Option<ls_types::GotoDefinitionResponse>> {
        let mut timer = RequestTimer::new("handler");
        let response = self
            .with_ready_snapshot(move |snapshot| {
                let (file, offset) = snapshot.position_for_document_request(
                    &params.text_document_position_params.text_document,
                    params.text_document_position_params.position,
                )?;
                let db = snapshot.db();

                if !matches!(file.try_source(db), Ok(source) if *source.kind() == FileKind::Template)
                {
                    return None;
                }

                djls_ide::goto_definition(
                    db,
                    file,
                    offset,
                    snapshot.client_info().supports_location_links(),
                    snapshot.client_info().position_encoding(),
                )
            })
            .await;

        let result_count = match &response {
            None => 0,
            Some(ls_types::GotoDefinitionResponse::Scalar(_)) => 1,
            Some(ls_types::GotoDefinitionResponse::Array(locations)) => locations.len(),
            Some(ls_types::GotoDefinitionResponse::Link(links)) => links.len(),
        };
        debug!(result_count);
        timer.finish_result(result_count == 0);
        Ok(response)
    }

    #[tracing::instrument(
        level = "debug",
        skip_all,
        name = "lsp.request",
        fields(method = "textDocument/references")
    )]
    async fn references(
        &self,
        params: ls_types::ReferenceParams,
    ) -> LspResult<Option<Vec<ls_types::Location>>> {
        let mut timer = RequestTimer::new("handler");
        let response = self
            .with_ready_snapshot(move |snapshot| {
                let (file, offset) = snapshot.position_for_document_request(
                    &params.text_document_position.text_document,
                    params.text_document_position.position,
                )?;
                let db = snapshot.db();

                if !matches!(file.try_source(db), Ok(source) if *source.kind() == FileKind::Template)
                {
                    return None;
                }

                djls_ide::find_references(
                    db,
                    file,
                    offset,
                    snapshot.client_info().position_encoding(),
                    params.context.include_declaration,
                )
            })
            .await;

        timer.finish_result(response.as_ref().is_none_or(Vec::is_empty));
        Ok(response)
    }

    #[tracing::instrument(
        level = "debug",
        skip_all,
        name = "lsp.request",
        fields(method = "textDocument/formatting")
    )]
    async fn formatting(
        &self,
        params: ls_types::DocumentFormattingParams,
    ) -> LspResult<Option<Vec<ls_types::TextEdit>>> {
        let mut timer = RequestTimer::new("handler");
        let edits = self
            .with_snapshot(move |snapshot| {
                let Some(file) = snapshot.file_for_document_request(&params.text_document) else {
                    return Vec::new();
                };
                let db = snapshot.db();
                let format_config = db.settings().format().clone();

                if !format_config.enabled() {
                    return Vec::new();
                }

                let Ok(source) = file.try_source(db) else {
                    return Vec::new();
                };
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

        debug!(result_count = edits.len());
        timer.finish_result(edits.is_empty());
        Ok(Some(edits))
    }

    #[tracing::instrument(
        level = "debug",
        skip_all,
        name = "lsp.notification",
        fields(method = "workspace/didChangeConfiguration")
    )]
    async fn did_change_configuration(&self, _params: ls_types::DidChangeConfigurationParams) {
        let mut timer = RequestTimer::new("handler");
        tracing::info!("Configuration change detected. Requesting project reload...");
        self.reload.request_full_reload().await;
        timer.finish("scheduled");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;
    use std::thread::sleep as sleep_thread;
    use std::time::Duration;

    use camino::Utf8PathBuf;
    use djls_ide::prime_template_library_products;
    use djls_source::path_to_file;
    use percent_encoding::percent_decode_str;
    use tokio::spawn as spawn_task;
    use tokio::time::timeout;
    use tracing::Instrument;
    use tracing::instrument::WithSubscriber;
    use tracing_subscriber::layer::SubscriberExt;

    use super::*;
    use crate::logging::capture::Capture;
    use crate::logging::capture::callsite_guard;
    use crate::session::ProjectWork;

    #[tokio::test]
    async fn observability_interleaved_requests_keep_scoped_context_and_private_values() {
        let _callsite_guard = callsite_guard();
        let first = Capture::default();
        let second = Capture::default();
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let run = |method, capture: Capture, barrier: Arc<tokio::sync::Barrier>| async move {
            let dispatch = tracing::Dispatch::new(tracing_subscriber::registry().with(capture));
            let span = tracing::dispatcher::with_default(&dispatch, || {
                tracing::debug_span!("lsp.request", method)
            });
            async move {
                let session = Arc::new(Mutex::new(Session::default()));
                barrier.wait().await;
                tokio::task::yield_now().await;
                let result = with_session_snapshot(
                    &session,
                    Arc::new(|snapshot: &SessionSnapshot| {
                        debug!(event = "worker_probe");
                        assert!(
                            snapshot
                                .file_for_document_request(&ls_types::TextDocumentIdentifier {
                                    uri: "untitled:PRIVATE_URI_SENTINEL"
                                        .parse()
                                        .expect("valid test URI"),
                                },)
                                .is_none()
                        );
                        "PRIVATE_RETURN_SENTINEL".to_string()
                    }),
                )
                .await;
                assert_eq!(result, "PRIVATE_RETURN_SENTINEL");
            }
            .instrument(span)
            .with_subscriber(dispatch)
            .await;
        };
        tokio::join!(
            run("first", first.clone(), Arc::clone(&barrier)),
            run("second", second.clone(), barrier)
        );
        for (capture, method) in [(first, "first"), (second, "second")] {
            let events = capture
                .0
                .lock()
                .expect("capture lock should not be poisoned");
            let probes: Vec<_> = events
                .iter()
                .filter(|event| event["fields"]["event"] == "worker_probe")
                .collect();
            assert_eq!(probes.len(), 1);
            assert_eq!(probes[0]["spans"][0]["fields"]["method"], method);
            assert_eq!(probes[0]["spans"][1]["name"], "session.syntax_snapshot");
            let terminal = events
                .iter()
                .find(|event| event["fields"]["event"] == "request_completed")
                .expect("snapshot should emit completion");
            assert_eq!(terminal["fields"]["outcome"], "success");
            assert!(terminal["fields"]["elapsed_ms"].is_f64());
            assert!(
                !serde_json::to_string(&*events)
                    .expect("captured JSON should serialize")
                    .contains("PRIVATE_")
            );
        }
    }

    #[tokio::test]
    async fn observability_fallback_outcomes_are_distinct() {
        let _callsite_guard = callsite_guard();
        let capture = Capture::default();
        async {
            let session = Arc::new(Mutex::new(Session::default()));
            assert_eq!(
                with_session_snapshot(&session, Arc::new(|_: &SessionSnapshot| 0_usize)).await,
                0
            );
            assert_eq!(
                with_session_snapshot(
                    &session,
                    Arc::new(|_: &SessionSnapshot| -> usize {
                        panic!("PRIVATE_PANIC_SENTINEL");
                    })
                )
                .await,
                0
            );
            assert_eq!(
                with_session_snapshot(
                    &session,
                    Arc::new(|_: &SessionSnapshot| -> usize {
                        std::panic::resume_unwind(Box::new(Cancelled::Local));
                    })
                )
                .await,
                0
            );
            assert!(session.lock().await.fail_intrinsic_readiness(0));
            assert_eq!(
                with_ready_session_snapshot(&session, Arc::new(|_: &SessionSnapshot| 19_usize))
                    .await,
                0
            );
        }
        .with_subscriber(tracing_subscriber::registry().with(capture.clone()))
        .await;
        let events = capture
            .0
            .lock()
            .expect("capture lock should not be poisoned");
        let outcomes: Vec<_> = events
            .iter()
            .filter(|event| event["fields"]["event"] == "request_completed")
            .map(|event| {
                event["fields"]["outcome"]
                    .as_str()
                    .expect("outcome should be a string")
            })
            .collect();
        assert_eq!(
            outcomes,
            [
                "success",
                "blocking_task_failed",
                "cancellation_exhausted",
                "readiness_failed"
            ]
        );
        let attempts: Vec<_> = events
            .iter()
            .filter_map(|event| event["fields"]["attempt"].as_u64())
            .collect();
        assert_eq!(attempts, [1, 2]);
        assert!(
            !serde_json::to_string(&*events)
                .expect("captured JSON should serialize")
                .contains("PRIVATE_")
        );
    }

    #[test]
    fn observability_compute_timing_excludes_blocking_pool_queue() {
        let _callsite_guard = callsite_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .expect("test runtime should build");
        let capture = Capture::default();
        runtime.block_on(
            async {
                let session = Arc::new(Mutex::new(Session::default()));
                let (release_tx, release_rx) = mpsc::channel();
                let (started_tx, started_rx) = tokio::sync::oneshot::channel();
                let blocker = spawn_blocking(move || {
                    started_tx.send(()).expect("start receiver should exist");
                    release_rx.recv().expect("release should arrive");
                });
                started_rx.await.expect("worker should start");
                let start = Instant::now();
                let work =
                    with_session_snapshot(&session, Arc::new(|_: &SessionSnapshot| 37_usize));
                tokio::pin!(work);
                assert!(timeout(Duration::from_millis(30), &mut work).await.is_err());
                let queue_ms = start.elapsed().as_secs_f64() * 1000.0;
                release_tx.send(()).expect("release receiver should exist");
                assert_eq!(work.await, 37);
                blocker.await.expect("blocker should finish");
                let events = capture
                    .0
                    .lock()
                    .expect("capture lock should not be poisoned");
                let compute_ms = events
                    .iter()
                    .find_map(|event| event["fields"]["compute_ms"].as_f64())
                    .expect("compute timing should be numeric");
                // Compare measured intervals rather than assuming a fast worker or a
                // fixed upper bound on scheduler latency.
                assert!(compute_ms < start.elapsed().as_secs_f64() * 1000.0 - queue_ms);
            }
            .with_subscriber(tracing_subscriber::registry().with(capture.clone())),
        );
    }

    #[tokio::test]
    async fn observability_readiness_wait_is_separate_from_compute_and_drop_is_cancelled() {
        let _callsite_guard = callsite_guard();
        let capture = Capture::default();
        let held_ms = async {
            let session = Arc::new(Mutex::new(Session::default()));
            let primed = prime_template_library_products(session.lock().await.db())
                .expect("session should have a project");
            {
                let request =
                    with_ready_session_snapshot(&session, Arc::new(|_: &SessionSnapshot| 41_usize));
                tokio::pin!(request);
                assert!(
                    timeout(Duration::from_millis(10), &mut request)
                        .await
                        .is_err()
                );
                // Dropping this pending request must not be reported as success.
            }
            let request =
                with_ready_session_snapshot(&session, Arc::new(|_: &SessionSnapshot| 43_usize));
            tokio::pin!(request);
            assert!(
                timeout(Duration::from_millis(10), &mut request)
                    .await
                    .is_err()
            );
            let held = Instant::now();
            tokio::time::sleep(Duration::from_millis(20)).await;
            let held_ms = held.elapsed().as_secs_f64() * 1000.0;
            assert!(session.lock().await.publish_intrinsic_readiness(0, &primed));
            assert_eq!(request.await, 43);
            held_ms
        }
        .with_subscriber(tracing_subscriber::registry().with(capture.clone()))
        .await;
        let events = capture
            .0
            .lock()
            .expect("capture lock should not be poisoned");
        let terminal: Vec<_> = events
            .iter()
            .filter(|event| event["fields"]["event"] == "request_completed")
            .collect();
        assert_eq!(terminal[0]["fields"]["outcome"], "cancelled");
        assert_eq!(terminal[1]["fields"]["outcome"], "success");
        let ready = events
            .iter()
            .find(|event| event["fields"]["event"] == "readiness_completed")
            .expect("readiness should emit completion");
        assert_eq!(ready["fields"]["generation"], 0);
        let ready_ms = ready["fields"]["ready_wait_ms"]
            .as_f64()
            .expect("readiness timing should be numeric");
        assert!(ready_ms >= held_ms);
        let compute_ms = events
            .iter()
            .find_map(|event| event["fields"]["compute_ms"].as_f64())
            .expect("compute timing should be numeric");
        assert!(
            terminal[1]["fields"]["elapsed_ms"]
                .as_f64()
                .expect("elapsed timing should be numeric")
                >= ready_ms + compute_ms
        );
    }

    #[tokio::test]
    async fn observability_handler_events_by_filter_and_privacy() {
        let _callsite_guard = callsite_guard();
        for (mode, filter) in [
            ("disabled", "off"),
            ("default", "warn,djls_server=info"),
            ("debug", "warn,djls_server=debug"),
        ] {
            let capture = Capture::default();
            let dispatch = tracing::Dispatch::new(
                tracing_subscriber::registry()
                    .with(capture.clone())
                    .with(tracing_subscriber::EnvFilter::new(filter)),
            );
            let (service, _socket) = tower_lsp_server::LspService::new(|client| {
                DjangoLanguageServer::new(client, LspLogControl::disconnected())
            });
            let server = service.inner();
            async {
                for _ in 0..20 {
                    server
                        .did_open(ls_types::DidOpenTextDocumentParams {
                            text_document: ls_types::TextDocumentItem {
                                uri: "untitled:PRIVATE_URI_SENTINEL"
                                    .parse()
                                    .expect("valid test URI"),
                                language_id: "PRIVATE_LANGUAGE_SENTINEL".into(),
                                text: "PRIVATE_SOURCE_SENTINEL".into(),
                                version: 1,
                            },
                        })
                        .await;
                    let result = server
                        .formatting(ls_types::DocumentFormattingParams {
                            text_document: ls_types::TextDocumentIdentifier {
                                uri: "untitled:PRIVATE_URI_SENTINEL"
                                    .parse()
                                    .expect("valid test URI"),
                            },
                            options: ls_types::FormattingOptions::default(),
                            work_done_progress_params: ls_types::WorkDoneProgressParams::default(),
                        })
                        .await
                        .expect("formatting should succeed");
                    assert_eq!(result, Some(Vec::new()));
                }
            }
            .with_subscriber(dispatch)
            .await;
            let events = capture
                .0
                .lock()
                .expect("capture lock should not be poisoned");
            assert!(
                !serde_json::to_string(&*events)
                    .expect("captured JSON should serialize")
                    .contains("PRIVATE_")
            );
            if mode == "debug" {
                let handlers: Vec<_> = events
                    .iter()
                    .filter(|event| {
                        event["fields"]["event"] == "request_completed"
                            && event["fields"]["operation"] == "handler"
                    })
                    .collect();
                assert_eq!(handlers.len(), 40);
                assert_eq!(
                    handlers
                        .iter()
                        .filter(|event| event["fields"]["outcome"] == "ignored")
                        .count(),
                    20
                );
                assert_eq!(
                    handlers
                        .iter()
                        .filter(|event| event["fields"]["outcome"] == "empty")
                        .count(),
                    20
                );
                for event in handlers {
                    assert!(event["fields"]["elapsed_ms"].is_f64());
                    assert!(matches!(
                        event["spans"][0]["fields"]["method"].as_str(),
                        Some("textDocument/didOpen" | "textDocument/formatting")
                    ));
                }
            } else {
                assert!(events.is_empty());
            }
        }
    }

    fn report_params() -> ReportUnreadableRegistrationParams {
        ReportUnreadableRegistrationParams {
            module: "app.templatetags.open_tags".to_string(),
            file: "file:///tmp/open_tags.py"
                .parse()
                .expect("test file URI should parse"),
            line: 7,
            shape: "the registered name cannot be resolved".to_string(),
            count: 1,
        }
    }

    fn issue_query_value(uri: &ls_types::Uri, name: &str) -> String {
        let query = uri
            .as_str()
            .split_once('?')
            .map(|(_, query)| query)
            .expect("issue URI should contain a query");
        let encoded = query
            .split('&')
            .find_map(|field| field.strip_prefix(&format!("{name}=")))
            .expect("issue URI should contain the requested query field");
        percent_decode_str(encoded)
            .decode_utf8()
            .expect("issue query should contain UTF-8")
            .into_owned()
    }

    #[test]
    fn unreadable_registration_issue_uri_encodes_markdown_query_values() {
        let uri = unreadable_registration_issue_uri(&report_params(), "# heading & detail\nnext")
            .expect("issue URI should build");

        assert!(uri.as_str().contains("%23"));
        assert!(uri.as_str().contains("%26"));
        assert!(uri.as_str().contains("%0A"));
        assert_eq!(
            issue_query_value(&uri, "title"),
            "Unreadable tag registration: the registered name cannot be resolved"
        );
        let body = issue_query_value(&uri, "body");
        assert!(body.contains("# heading & detail\nnext"));
        assert!(body.contains("unrecognized tags and filters from that library are not reported"));
    }

    #[test]
    fn unreadable_registration_issue_uri_omits_overlong_encoded_statement() {
        let statement = "# &\n".repeat(2_000);
        let uri = unreadable_registration_issue_uri(&report_params(), &statement)
            .expect("issue URI should build");
        let body = issue_query_value(&uri, "body");

        assert!(body.contains("statement omitted, too long"));
        assert!(!body.contains("# &\n# &"));
        let encoded_body = uri
            .as_str()
            .split_once("&body=")
            .map(|(_, body)| body)
            .expect("issue URI should contain an encoded body");
        assert!(encoded_body.len() <= MAX_ENCODED_ISSUE_BODY_BYTES);
    }

    #[test]
    fn unreadable_registration_issue_uri_rejects_overlong_metadata_body() {
        let mut params = report_params();
        params.module = "m".repeat(MAX_ENCODED_ISSUE_BODY_BYTES);

        assert!(unreadable_registration_issue_uri(&params, "short statement").is_none());
    }

    #[test]
    fn statement_text_uses_only_the_statement_span_and_clamps_to_forty_lines() {
        let source = format!("before\n{}after\n", "part\n".repeat(45));
        let start = "before\n".len();
        let length = "part\n".repeat(45).len();
        let span = Span::saturating_from_parts_usize(start, length);
        let statement = statement_text(&source, span).expect("statement span should be valid");

        assert_eq!(statement.lines().count(), MAX_STATEMENT_LINES);
        assert!(statement.lines().all(|line| line == "part"));
        assert!(!statement.contains("before"));
        assert!(!statement.contains("after"));
    }

    #[test]
    fn report_unreadable_registration_params_reject_bad_arguments() {
        assert!(parse_report_unreadable_registration_params(&[]).is_err());
        assert!(
            parse_report_unreadable_registration_params(&[serde_json::json!("not an object")])
                .is_err()
        );
        assert!(
            parse_report_unreadable_registration_params(&[serde_json::json!({
                "module": "app.tags",
                "file": "file:///tmp/tags.py",
                "line": 1,
                "shape": "unknown",
                "count": 1,
                "extra": true,
            })])
            .is_err()
        );
        let mut invalid_line = report_params().into_lsp_value();
        invalid_line["line"] = serde_json::json!(0);
        assert!(parse_report_unreadable_registration_params(&[invalid_line]).is_err());
    }

    #[tokio::test]
    async fn syntax_only_request_task_panic_returns_default() {
        let session = Arc::new(Mutex::new(Session::default()));
        let executions = Arc::new(AtomicUsize::new(0));

        let response: usize = with_session_snapshot(&session, {
            let executions = Arc::clone(&executions);
            Arc::new(move |_: &SessionSnapshot| {
                executions.fetch_add(1, Ordering::SeqCst);
                panic!("synthetic syntax request panic");
            })
        })
        .await;

        assert_eq!(response, usize::default());
        assert_eq!(executions.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn project_aware_request_task_panic_returns_default() {
        let session = Arc::new(Mutex::new(Session::default()));
        let executions = Arc::new(AtomicUsize::new(0));
        let mut request = spawn_task({
            let session = Arc::clone(&session);
            let executions = Arc::clone(&executions);
            async move {
                with_ready_session_snapshot(
                    &session,
                    Arc::new(move |_: &SessionSnapshot| {
                        executions.fetch_add(1, Ordering::SeqCst);
                        panic!("synthetic project-aware request panic");
                    }),
                )
                .await
            }
        });

        assert!(
            timeout(Duration::from_millis(20), &mut request)
                .await
                .is_err(),
            "an unready generation must block project-aware requests"
        );
        assert_eq!(executions.load(Ordering::SeqCst), 0);

        let primed = {
            let session = session.lock().await;
            prime_template_library_products(session.db())
                .expect("default session should have a project")
        };
        assert!(session.lock().await.publish_intrinsic_readiness(0, &primed));

        let response: usize = timeout(Duration::from_secs(1), request)
            .await
            .expect("project-aware request should finish before the test timeout")
            .expect("project-aware request task should finish successfully");
        assert_eq!(response, usize::default());
        assert_eq!(executions.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancellation_restarts_at_barrier_and_waits_for_reprime() {
        let session = Arc::new(Mutex::new(Session::default()));
        let path = Utf8PathBuf::from("/tmp/retry.py");
        let uri: ls_types::Uri = "file:///tmp/retry.py".parse().expect("valid test URI");
        let generation = {
            let mut session = session.lock().await;
            match session.open_document(&ls_types::TextDocumentItem {
                uri: uri.clone(),
                language_id: "python".to_string(),
                version: 1,
                text: "initial".to_string(),
            }) {
                DocumentMutation::Applied { .. } => Some(()),
                DocumentMutation::Ignored => None,
            }
            .expect("Python test document should open");
            let file = path_to_file(session.db(), &path).expect("open document should be interned");
            let generation = session.desired_generation();
            let primed = prime_template_library_products(session.db())
                .expect("session should have a project");
            assert!(session.publish_intrinsic_readiness(generation, &primed));
            session.install_ready_coverage_for_test(vec![file], Vec::new());
            generation
        };
        let attempts = Arc::new(AtomicUsize::new(0));
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Arc::new(StdMutex::new(release_rx));
        let mut request = spawn_task({
            let session = Arc::clone(&session);
            let attempts = Arc::clone(&attempts);
            let release_rx = Arc::clone(&release_rx);
            let path = path.clone();
            async move {
                with_ready_session_snapshot(
                    &session,
                    Arc::new(move |snapshot: &SessionSnapshot| {
                        let attempt = attempts.fetch_add(1, Ordering::SeqCst) + 1;
                        if attempt == 1 {
                            started_tx.send(()).expect("start receiver dropped");
                            let release = release_rx.lock().expect("release mutex poisoned");
                            release.recv().expect("release sender dropped");
                            sleep_thread(Duration::from_millis(50));
                        }
                        path_to_file(snapshot.db(), &path)
                            .expect("snapshot should contain the open Python document")
                            .try_source(snapshot.db())
                            .expect("snapshot Python source should be readable")
                            .as_str()
                            .to_string()
                    }),
                )
                .await
            }
        });
        spawn_blocking(move || started_rx.recv().expect("start signal should arrive"))
            .await
            .expect("start waiter should finish successfully");
        release_tx.send(()).expect("release receiver dropped");
        let replacement_generation = {
            let mut session = session.lock().await;
            let project_work = match session.update_document(
                &ls_types::VersionedTextDocumentIdentifier { uri, version: 2 },
                vec![ls_types::TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: "updated".to_string(),
                }],
            ) {
                DocumentMutation::Applied { project_work, .. } => Some(project_work),
                DocumentMutation::Ignored => None,
            }
            .expect("open Python test document should update");
            assert_eq!(project_work, Some(ProjectWork::Reprime));
            session.desired_generation()
        };
        assert_eq!(replacement_generation, generation + 1);
        assert!(
            timeout(Duration::from_millis(20), &mut request)
                .await
                .is_err(),
            "cancelled request must wait at the new generation barrier"
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        let current_prime = {
            let session = session.lock().await;
            prime_template_library_products(session.db()).expect("session should have a project")
        };
        assert!(
            session
                .lock()
                .await
                .publish_intrinsic_readiness(replacement_generation, &current_prime)
        );
        let response = timeout(Duration::from_secs(1), request)
            .await
            .expect("retried request should finish before the test timeout")
            .expect("retried request task should finish successfully");
        assert_eq!(response, "updated");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn final_state_matrix_03_project_requests_wait_for_current_generation() {
        let session = Arc::new(Mutex::new(Session::default()));
        let mut initial_waiter = spawn_task({
            let session = Arc::clone(&session);
            async move { await_ready_session_snapshot(&session).await }
        });
        assert!(
            timeout(Duration::from_millis(20), &mut initial_waiter)
                .await
                .is_err(),
            "an unready generation must block project-aware requests"
        );

        let initial_prime = {
            let session = session.lock().await;
            prime_template_library_products(session.db())
                .expect("default session should have a project")
        };
        assert!(
            session
                .lock()
                .await
                .publish_intrinsic_readiness(0, &initial_prime)
        );
        assert_eq!(
            timeout(Duration::from_secs(1), initial_waiter)
                .await
                .expect("initial readiness waiter should finish before the test timeout")
                .expect("initial readiness waiter task should finish successfully")
                .expect("initial ready snapshot should be available")
                .intrinsic_generation(),
            Some(0)
        );

        let generation = session.lock().await.mark_project_changed();
        let mut replacement_waiter = spawn_task({
            let session = Arc::clone(&session);
            async move { await_ready_session_snapshot(&session).await }
        });
        assert!(
            timeout(Duration::from_millis(20), &mut replacement_waiter)
                .await
                .is_err()
        );
        assert!(
            !session
                .lock()
                .await
                .publish_intrinsic_readiness(0, &initial_prime),
            "stale completion must not publish readiness"
        );

        let current_prime = {
            let session = session.lock().await;
            prime_template_library_products(session.db())
                .expect("default session should have a project")
        };
        assert!(
            session
                .lock()
                .await
                .publish_intrinsic_readiness(generation, &current_prime)
        );
        assert_eq!(
            timeout(Duration::from_secs(1), replacement_waiter)
                .await
                .expect("replacement readiness waiter should finish before the test timeout")
                .expect("replacement readiness waiter task should finish successfully")
                .expect("replacement ready snapshot should be available")
                .intrinsic_generation(),
            Some(generation)
        );
    }
}

use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use djls_ide::REPORT_UNREADABLE_REGISTRATION_COMMAND;
use djls_ide::ReportUnreadableRegistrationParams;
use djls_project::ScopedTemplateLibraries;
use djls_project::template_library_catalog;
use djls_project::template_library_definition_facts;
use djls_source::FileKind;
use djls_source::Span;
use djls_source::path_to_file;
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

use crate::document::TextDocument;
use crate::ext::PositionEncodingExt;
use crate::ext::UriExt;
use crate::logging::LoggingGuard;
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
    logging: LoggingGuard,
}

impl DjangoLanguageServer {
    #[must_use]
    pub(crate) fn new(client: Client, logging: LoggingGuard) -> Self {
        let session = Arc::new(Mutex::new(Session::default()));
        let reload = ProjectReload::new(Arc::clone(&session), client.clone());

        Self {
            client,
            session,
            reload,
            logging,
        }
    }

    async fn with_session<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Session) -> R,
    {
        let session = self.session.lock().await;
        f(&session)
    }

    async fn with_session_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut Session) -> R,
    {
        let mut session = self.session.lock().await;
        f(&mut session)
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

    fn schedule_document_mutation(&self, mutation: DocumentMutation) -> Option<TextDocument> {
        match mutation {
            DocumentMutation::Ignored => None,
            DocumentMutation::Applied {
                document,
                project_work,
            } => {
                if let Some(project_work) = project_work {
                    self.reload.request_current(project_work);
                }
                Some(document)
            }
        }
    }

    async fn maybe_push_diagnostics(&self, document: &TextDocument) {
        if self
            .with_session(|session| session.client_info().supports_pull_diagnostics())
            .await
        {
            debug!("Client supports pull diagnostics, skipping push");
            return;
        }

        let path = document.path().to_path_buf();
        let Some(diagnostics) = self
            .with_ready_snapshot(move |snapshot| {
                let file = path_to_file(snapshot.db(), &path).ok()?;
                djls_ide::collect_diagnostics(
                    snapshot.db(),
                    file,
                    snapshot.client_info().position_encoding(),
                )
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

        debug!(
            "Published {} diagnostics for {}",
            diagnostic_count, lsp_uri_text
        );
    }
}

async fn with_session_snapshot<F, R>(session: &Arc<Mutex<Session>>, f: Arc<F>) -> R
where
    F: Fn(&SessionSnapshot) -> R + Send + Sync + 'static,
    R: Default + Send + 'static,
{
    let mut retry_state = CancellationRetryState::new();
    loop {
        let snapshot = { session.lock().await.snapshot() };
        let f = Arc::clone(&f);
        let result =
            match spawn_blocking(move || Cancelled::catch(AssertUnwindSafe(|| f(&snapshot)))).await
            {
                Ok(result) => result,
                Err(error) => {
                    error!(
                        ?error,
                        "Syntax-only request snapshot task failed; returning fallback"
                    );
                    return R::default();
                }
            };
        match result {
            Ok(result) => return result,
            Err(cancelled) => match retry_state.after_cancellation() {
                CancellationRetryAction::Retry { attempt } => {
                    debug!(?cancelled, attempt, "Syntax snapshot cancelled; retrying");
                }
                CancellationRetryAction::Exhausted => {
                    debug!(?cancelled, "Syntax snapshot cancelled; returning fallback");
                    return R::default();
                }
            },
        }
    }
}

async fn with_ready_session_snapshot<F, R>(session: &Arc<Mutex<Session>>, f: Arc<F>) -> R
where
    F: Fn(&SessionSnapshot) -> R + Send + Sync + 'static,
    R: Default + Send + 'static,
{
    let mut retry_state = CancellationRetryState::new();
    loop {
        let Some(snapshot) = await_ready_session_snapshot(session).await else {
            return R::default();
        };
        let f = Arc::clone(&f);
        let result =
            match spawn_blocking(move || Cancelled::catch(AssertUnwindSafe(|| f(&snapshot)))).await
            {
                Ok(result) => result,
                Err(error) => {
                    error!(
                        ?error,
                        "Project-aware request snapshot task failed; returning fallback"
                    );
                    return R::default();
                }
            };

        match result {
            Ok(result) => return result,
            Err(cancelled) => match retry_state.after_cancellation() {
                CancellationRetryAction::Retry { attempt } => {
                    debug!(
                        ?cancelled,
                        attempt, "Snapshot request cancelled; retrying from intrinsic readiness"
                    );
                }
                CancellationRetryAction::Exhausted => {
                    debug!(
                        ?cancelled,
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
    let mut readiness = { session.lock().await.readiness_receiver() };
    loop {
        let observed = *readiness.borrow_and_update();
        match observed {
            IntrinsicReadinessState::Unready(_) => {
                if readiness.changed().await.is_err() {
                    return None;
                }
            }
            IntrinsicReadinessState::Failed(generation) => {
                let session = session.lock().await;
                if session.readiness_state() == IntrinsicReadinessState::Failed(generation) {
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

    async fn initialized(&self, _params: ls_types::InitializedParams) {
        tracing::info!("Server received initialized notification.");

        self.reload.request_full_reload().await;
    }

    async fn shutdown(&self) -> LspResult<()> {
        self.logging.disable_lsp();
        Ok(())
    }

    async fn did_open(&self, params: ls_types::DidOpenTextDocumentParams) {
        let mutation = self
            .with_session_mut(|session| session.open_document(&params.text_document))
            .await;

        if let Some(document) = self.schedule_document_mutation(mutation) {
            self.maybe_push_diagnostics(&document).await;
        }
    }

    async fn did_save(&self, params: ls_types::DidSaveTextDocumentParams) {
        let mutation = self
            .with_session_mut(|session| session.save_document(&params.text_document))
            .await;

        if let Some(document) = self.schedule_document_mutation(mutation) {
            self.maybe_push_diagnostics(&document).await;
        }
    }

    async fn did_change(&self, params: ls_types::DidChangeTextDocumentParams) {
        let mutation = self
            .with_session_mut(|session| {
                session.update_document(&params.text_document, params.content_changes)
            })
            .await;

        if let Some(document) = self.schedule_document_mutation(mutation) {
            self.maybe_push_diagnostics(&document).await;
        }
    }

    async fn did_close(&self, params: ls_types::DidCloseTextDocumentParams) {
        let mutation = self
            .with_session_mut(|session| session.close_document(&params.text_document))
            .await;
        drop(self.schedule_document_mutation(mutation));
    }

    async fn code_action(
        &self,
        params: ls_types::CodeActionParams,
    ) -> LspResult<Option<ls_types::CodeActionResponse>> {
        if params.context.only.as_ref().is_some_and(|only| {
            !only
                .iter()
                .any(|kind| kind == &ls_types::CodeActionKind::QUICKFIX)
        }) {
            return Ok(None);
        }

        let response = self
            .with_ready_snapshot(move |snapshot| {
                let (file, range) = snapshot.range_for_document_request(
                    &params.text_document,
                    params.range,
                    "code action",
                )?;
                let db = snapshot.db();

                if !matches!(file.try_source(db), Ok(source) if *source.kind() == FileKind::Template)
                {
                    return None;
                }

                djls_ide::code_actions(db, file, range, snapshot.client_info().position_encoding())
            })
            .await;

        Ok(response)
    }

    async fn execute_command(
        &self,
        params: ls_types::ExecuteCommandParams,
    ) -> LspResult<Option<serde_json::Value>> {
        if params.command != REPORT_UNREADABLE_REGISTRATION_COMMAND {
            return Err(LspError::invalid_params(format!(
                "unknown command: {}",
                params.command
            )));
        }
        let report = parse_report_unreadable_registration_params(&params.arguments)
            .map_err(LspError::invalid_params)?;
        let issue_uri = self
            .with_ready_snapshot(move |snapshot| {
                report_unreadable_registration_issue_uri(snapshot.db(), &report)
            })
            .await
            .ok_or_else(|| {
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
            .await?;

        Ok(None)
    }

    async fn completion(
        &self,
        params: ls_types::CompletionParams,
    ) -> LspResult<Option<ls_types::CompletionResponse>> {
        let response = self
            .with_ready_snapshot(move |snapshot| {
                let (file, offset) = snapshot.position_for_document_request(
                    &params.text_document_position.text_document,
                    params.text_document_position.position,
                    "completion",
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

        Ok(response)
    }

    async fn hover(&self, params: ls_types::HoverParams) -> LspResult<Option<ls_types::Hover>> {
        let response = self
            .with_ready_snapshot(move |snapshot| {
                let (file, offset) = snapshot.position_for_document_request(
                    &params.text_document_position_params.text_document,
                    params.text_document_position_params.position,
                    "hover",
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

        Ok(response)
    }

    async fn diagnostic(
        &self,
        params: ls_types::DocumentDiagnosticParams,
    ) -> LspResult<ls_types::DocumentDiagnosticReportResult> {
        debug!(
            "Received diagnostic request for {:?}",
            params.text_document.uri
        );

        let diagnostics = self
            .with_ready_snapshot(move |snapshot| {
                let Some(file) =
                    snapshot.file_for_document_request(&params.text_document, "diagnostic")
                else {
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
            .with_ready_snapshot(move |snapshot| {
                let Some(file) =
                    snapshot.file_for_document_request(&params.text_document, "folding")
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

        Ok(Some(ranges))
    }

    async fn document_symbol(
        &self,
        params: ls_types::DocumentSymbolParams,
    ) -> LspResult<Option<ls_types::DocumentSymbolResponse>> {
        let symbols = self
            .with_ready_snapshot(move |snapshot| {
                let Some(file) =
                    snapshot.file_for_document_request(&params.text_document, "document symbol")
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

        Ok(Some(ls_types::DocumentSymbolResponse::Nested(symbols)))
    }

    async fn document_link(
        &self,
        params: ls_types::DocumentLinkParams,
    ) -> LspResult<Option<Vec<ls_types::DocumentLink>>> {
        let links = self
            .with_ready_snapshot(move |snapshot| {
                let Some(file) =
                    snapshot.file_for_document_request(&params.text_document, "document link")
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

        Ok(Some(links))
    }

    async fn goto_definition(
        &self,
        params: ls_types::GotoDefinitionParams,
    ) -> LspResult<Option<ls_types::GotoDefinitionResponse>> {
        let response = self
            .with_ready_snapshot(move |snapshot| {
                let (file, offset) = snapshot.position_for_document_request(
                    &params.text_document_position_params.text_document,
                    params.text_document_position_params.position,
                    "goto definition",
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

        Ok(response)
    }

    async fn references(
        &self,
        params: ls_types::ReferenceParams,
    ) -> LspResult<Option<Vec<ls_types::Location>>> {
        let response = self
            .with_ready_snapshot(move |snapshot| {
                let (file, offset) = snapshot.position_for_document_request(
                    &params.text_document_position.text_document,
                    params.text_document_position.position,
                    "references",
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

        Ok(Some(edits))
    }

    async fn did_change_configuration(&self, _params: ls_types::DidChangeConfigurationParams) {
        tracing::info!("Configuration change detected. Requesting project reload...");
        self.reload.request_full_reload().await;
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
    use percent_encoding::percent_decode_str;
    use tokio::spawn as spawn_task;
    use tokio::time::timeout;

    use super::*;
    use crate::session::ProjectWork;

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

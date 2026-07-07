use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use djls_source::FileKind;
use djls_source::path_to_file;
use tokio::sync::Mutex;
use tower_lsp_server::Client;
use tower_lsp_server::LanguageServer;
use tower_lsp_server::jsonrpc::Result as LspResult;
use tower_lsp_server::ls_types;

use crate::document::TextDocument;
use crate::ext::PositionEncodingExt;
use crate::ext::UriExt;
use crate::logging::LoggingGuard;
use crate::reload::ProjectReload;
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

    /// Capture a snapshot under a brief lock, then compute on the blocking
    /// pool so the single-threaded event loop stays responsive.
    async fn with_snapshot<F, R>(&self, f: F) -> R
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

    async fn maybe_push_diagnostics(&self, document: &TextDocument) {
        if self
            .with_session(|session| session.client_info().supports_pull_diagnostics())
            .await
        {
            tracing::debug!("Client supports pull diagnostics, skipping push");
            return;
        }

        let path = document.path().to_path_buf();
        let Some(diagnostics) = self
            .with_snapshot(move |snapshot| {
                let file = path_to_file(snapshot.db(), &path).ok()?;
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

        self.reload.request();
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
            .with_snapshot(move |snapshot| {
                let (file, range) = snapshot.range_for_document_request(
                    &params.text_document,
                    params.range,
                    "code action",
                )?;
                let db = snapshot.db();

                if *file.source(db).kind() != FileKind::Template {
                    return None;
                }

                djls_ide::code_actions(db, file, range, snapshot.client_info().position_encoding())
            })
            .await;

        Ok(response)
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

    async fn document_link(
        &self,
        params: ls_types::DocumentLinkParams,
    ) -> LspResult<Option<Vec<ls_types::DocumentLink>>> {
        let links = self
            .with_snapshot(move |snapshot| {
                let Some(file) =
                    snapshot.file_for_document_request(&params.text_document, "document link")
                else {
                    return Vec::new();
                };
                let db = snapshot.db();

                if *file.source(db).kind() != FileKind::Template {
                    return Vec::new();
                }

                djls_ide::document_links(db, file)
            })
            .await;

        Ok(Some(links))
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

                djls_ide::goto_definition(
                    db,
                    file,
                    offset,
                    snapshot.client_info().supports_location_links(),
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
        tracing::info!("Configuration change detected. Requesting project reload...");
        self.reload.request();
    }
}

use std::sync::Arc;

use djls_semantic::Db as SemanticDb;
use djls_semantic::ProjectDb;
use djls_source::Db as SourceDb;
use djls_source::FileKind;
use djls_workspace::TextDocument;
use tokio::sync::Mutex;
use tower_lsp_server::jsonrpc::Result as LspResult;
use tower_lsp_server::ls_types;
use tower_lsp_server::Client;
use tower_lsp_server::LanguageServer;

use crate::ext::PositionEncodingExt;
use crate::ext::UriExt;
use crate::logging::LoggingGuard;
use crate::session::Session;
use crate::startup::run_startup_source_files;
use crate::startup::StartupController;
use crate::startup::StartupProgress;
use crate::startup::StartupRunInputs;

pub(crate) struct DjangoLanguageServer {
    client: Client,
    session: Arc<Mutex<Session>>,
    startup: StartupController,
    logging: LoggingGuard,
}

impl DjangoLanguageServer {
    #[must_use]
    pub(crate) fn new(client: Client, logging: LoggingGuard) -> Self {
        Self {
            client,
            session: Arc::new(Mutex::new(Session::default())),
            startup: StartupController::new(),
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

    async fn start_project_loading(
        &self,
    ) -> tokio::task::JoinHandle<crate::startup::StartupRunOutcome> {
        let guard = self.startup.start_generation().await;
        let generation = guard.generation();
        let inputs = {
            let session = self.session.lock().await;
            let progress = StartupProgress::for_client(
                self.client.clone(),
                session.client_info().supports_work_done_progress(),
                generation,
            );
            StartupRunInputs::capture_with_progress(&session, guard, progress)
        };
        let session = Arc::clone(&self.session);
        tokio::spawn(async move {
            let outcome = run_startup_source_files(session, inputs).await;
            tracing::debug!(?outcome, "Project loading run finished");
            outcome
        })
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
            .with_session(|session| {
                let db = session.db();
                let file = db.get_or_create_file(&path);
                djls_ide::collect_diagnostics(db, file)
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
        tracing::debug!(
            workspace_roots = session.workspace_roots().len(),
            work_done_progress = session.client_info().supports_work_done_progress(),
            "Captured workspace roots"
        );
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
        let _loading = self.start_project_loading().await;
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
            .with_session(|session| {
                let position = params.text_document_position.position;
                let encoding = session.client_info().position_encoding();
                let file = session.file_for_document_request(
                    &params.text_document_position.text_document,
                    "completion",
                )?;
                let db = session.db();
                let path = file.path(db);
                let source = file.source(db);
                let file_kind = *source.kind();

                tracing::debug!("Completion requested for {} at {:?}", path, position);
                let template_libraries = db.project().map(|project| project.template_libraries(db));

                let tag_specs = db.tag_specs();
                let supports_snippets = session.client_info().supports_snippets();

                // Compute position-aware available symbols for load-scoped completions.
                let available_symbols = if file_kind == FileKind::Template {
                    if let Some(template_libraries) = template_libraries {
                        if template_libraries.active_knowledge == djls_semantic::Knowledge::Known {
                            let nodelist = djls_templates::parse_template(db, file);

                            nodelist.map(|nl| {
                                let line_index = file.line_index(db);
                                let source_text = file.source(db);
                                let byte_offset = line_index.offset(
                                    source_text.as_str(),
                                    djls_source::LineCol::new(position.line, position.character),
                                    encoding,
                                );
                                djls_semantic::available_symbols_at(db, nl, byte_offset.get())
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
                    source.as_str(),
                    position,
                    encoding,
                    file_kind,
                    template_libraries,
                    Some(tag_specs),
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
                let db = session.db();

                if *file.source(db).kind() != FileKind::Template {
                    return Vec::new();
                }

                djls_ide::collect_diagnostics(db, file)
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
                let project_root = session.configuration_root();

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

        let loading = if settings_update.env_changed {
            Some(self.start_project_loading().await)
        } else {
            None
        };

        if let Some(loading) = loading {
            let _outcome = loading.await;
        }

        if settings_update.env_changed || settings_update.diagnostics_changed {
            self.republish_open_template_diagnostics().await;
        }
    }
}

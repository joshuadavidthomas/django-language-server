use std::future::Future;
use std::sync::Arc;

use camino::Utf8PathBuf;
use djls_project::Db as ProjectDb;
use djls_semantic::Db as SemanticDb;
use djls_source::FileKind;
use djls_workspace::paths;
use djls_workspace::LanguageId;
use tokio::sync::Mutex;
use tower_lsp_server::jsonrpc::Result as LspResult;
use tower_lsp_server::lsp_types;
use tower_lsp_server::Client;
use tower_lsp_server::LanguageServer;
use tracing_appender::non_blocking::WorkerGuard;
use url::Url;

use crate::encoding::LspPositionEncoding;
use crate::queue::Queue;
use crate::session::Session;

const SERVER_NAME: &str = "Django Language Server";
const SERVER_VERSION: &str = "0.1.0";

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
        F: FnOnce(Arc<Mutex<Session>>) -> Fut + Send + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        let session_arc = Arc::clone(&self.session);

        if let Err(e) = self.queue.submit(async move { f(session_arc).await }).await {
            tracing::error!("Failed to submit task: {}", e);
        } else {
            tracing::info!("Task submitted successfully");
        }
    }

    async fn publish_diagnostics(&self, url: &Url, version: Option<i32>) {
        // Check if client supports pull diagnostics - if so, don't push
        let supports_pull = self
            .with_session(super::session::Session::supports_pull_diagnostics)
            .await;

        if supports_pull {
            tracing::debug!(
                "Client supports pull diagnostics, skipping push for {}",
                url
            );
            return;
        }

        let Some(path) = paths::url_to_path(url) else {
            tracing::debug!("Could not convert URL to path: {}", url);
            return;
        };

        if FileKind::from(&path) != FileKind::Template {
            return;
        }

        let diagnostics: Vec<lsp_types::Diagnostic> = self
            .with_session_mut(|session| {
                let file = session.get_or_create_file(&path);
                session.with_db(|db| {
                    let nodelist = djls_templates::parse_template(db, file);
                    djls_ide::collect_diagnostics(db, file, nodelist)
                })
            })
            .await;

        let Some(lsp_uri) = paths::url_to_lsp_uri(url) else {
            tracing::debug!("Could not convert URL to LSP Uri: {}", url);
            return;
        };

        self.client
            .publish_diagnostics(lsp_uri, diagnostics.clone(), version)
            .await;

        tracing::debug!("Published {} diagnostics for {}", diagnostics.len(), url);
    }
}

impl LanguageServer for DjangoLanguageServer {
    async fn initialize(
        &self,
        params: lsp_types::InitializeParams,
    ) -> LspResult<lsp_types::InitializeResult> {
        tracing::info!("Initializing server...");

        let session = Session::new(&params);
        let encoding = session.position_encoding();

        {
            let mut session_lock = self.session.lock().await;
            *session_lock = session;
        }

        Ok(lsp_types::InitializeResult {
            capabilities: lsp_types::ServerCapabilities {
                completion_provider: Some(lsp_types::CompletionOptions {
                    resolve_provider: Some(false),
                    trigger_characters: Some(vec![
                        "{".to_string(),
                        "%".to_string(),
                        " ".to_string(),
                    ]),
                    ..Default::default()
                }),
                workspace: Some(lsp_types::WorkspaceServerCapabilities {
                    workspace_folders: Some(lsp_types::WorkspaceFoldersServerCapabilities {
                        supported: Some(true),
                        change_notifications: Some(lsp_types::OneOf::Left(true)),
                    }),
                    file_operations: None,
                }),
                text_document_sync: Some(lsp_types::TextDocumentSyncCapability::Options(
                    lsp_types::TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(lsp_types::TextDocumentSyncKind::INCREMENTAL),
                        will_save: Some(false),
                        will_save_wait_until: Some(false),
                        save: Some(lsp_types::SaveOptions::default().into()),
                    },
                )),
                position_encoding: Some(LspPositionEncoding::from(encoding).into()),
                diagnostic_provider: Some(lsp_types::DiagnosticServerCapabilities::Options(
                    lsp_types::DiagnosticOptions {
                        identifier: None,
                        inter_file_dependencies: false,
                        workspace_diagnostics: false,
                        work_done_progress_options: lsp_types::WorkDoneProgressOptions::default(),
                    },
                )),
                definition_provider: Some(lsp_types::OneOf::Left(true)),
                references_provider: Some(lsp_types::OneOf::Left(true)),
                ..Default::default()
            },
            server_info: Some(lsp_types::ServerInfo {
                name: SERVER_NAME.to_string(),
                version: Some(SERVER_VERSION.to_string()),
            }),
            offset_encoding: Some(encoding.to_string()),
        })
    }

    async fn initialized(&self, _params: lsp_types::InitializedParams) {
        tracing::info!("Server received initialized notification.");

        self.with_session_task(move |session_arc| async move {
            let session = session_arc.lock().await;

            if let Some(project) = session.project() {
                let path = project.root(session.database()).clone();
                tracing::info!("Task: Starting initialization for project at: {}", path);
                project.initialize(session.database());
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

    async fn did_open(&self, params: lsp_types::DidOpenTextDocumentParams) {
        tracing::info!("Opened document: {:?}", params.text_document.uri);

        let url_version = self
            .with_session_mut(|session| {
                let url =
                    paths::parse_lsp_uri(&params.text_document.uri, paths::LspContext::DidOpen)?;
                let document = djls_workspace::TextDocument::new(
                    params.text_document.text.clone(),
                    params.text_document.version,
                    LanguageId::from(params.text_document.language_id.as_str()),
                );

                session.open_document(&url, document);

                Some((url, params.text_document.version))
            })
            .await;

        if let Some((url, version)) = url_version {
            self.publish_diagnostics(&url, Some(version)).await;
        }
    }

    async fn did_save(&self, params: lsp_types::DidSaveTextDocumentParams) {
        tracing::info!("Saved document: {:?}", params.text_document.uri);

        let url_version = self
            .with_session_mut(|session| {
                let url =
                    paths::parse_lsp_uri(&params.text_document.uri, paths::LspContext::DidSave)?;
                let version = session.save_document(&url).map(|doc| doc.version());
                Some((url, version))
            })
            .await;

        if let Some((url, version)) = url_version {
            self.publish_diagnostics(&url, version).await;
        }
    }

    async fn did_change(&self, params: lsp_types::DidChangeTextDocumentParams) {
        tracing::info!("Changed document: {:?}", params.text_document.uri);

        self.with_session_mut(|session| {
            let url =
                paths::parse_lsp_uri(&params.text_document.uri, paths::LspContext::DidChange)?;
            session.update_document(&url, params.content_changes, params.text_document.version);
            Some(url)
        })
        .await;
    }

    async fn did_close(&self, params: lsp_types::DidCloseTextDocumentParams) {
        tracing::info!("Closed document: {:?}", params.text_document.uri);

        let url = self
            .with_session_mut(|session| {
                let url =
                    paths::parse_lsp_uri(&params.text_document.uri, paths::LspContext::DidClose)?;
                if session.close_document(&url).is_none() {
                    tracing::warn!("Attempted to close document without overlay: {}", url);
                }
                Some(url)
            })
            .await;

        // Clear diagnostics when closing a template file
        if let Some(url) = url {
            if let Some(path) = paths::url_to_path(&url) {
                if FileKind::from(&path) == FileKind::Template {
                    let Some(lsp_uri) = paths::url_to_lsp_uri(&url) else {
                        tracing::debug!("Could not convert URL to LSP Uri: {}", url);
                        return;
                    };

                    // Publish empty diagnostics to clear them (this method doesn't return a Result)
                    self.client.publish_diagnostics(lsp_uri, vec![], None).await;
                    tracing::debug!("Cleared diagnostics for {}", url);
                }
            }
        }
    }

    async fn completion(
        &self,
        params: lsp_types::CompletionParams,
    ) -> LspResult<Option<lsp_types::CompletionResponse>> {
        let response = self
            .with_session_mut(|session| {
                let Some(url) = paths::parse_lsp_uri(
                    &params.text_document_position.text_document.uri,
                    paths::LspContext::Completion,
                ) else {
                    return None; // Error parsing uri (unlikely), return no completions
                };

                tracing::debug!(
                    "Completion requested for {} at {:?}",
                    url,
                    params.text_document_position.position
                );

                if let Some(path) = paths::url_to_path(&url) {
                    let document = session.get_document(&url)?;
                    let position = params.text_document_position.position;
                    let encoding = session.position_encoding();
                    let file_kind = FileKind::from(&path);
                    let template_tags = session.with_db(|db| {
                        if let Some(project) = db.project() {
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
                        }
                    });
                    let tag_specs = session.with_db(SemanticDb::tag_specs);
                    let supports_snippets = session.supports_snippets();

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
                        Some(lsp_types::CompletionResponse::Array(completions))
                    }
                } else {
                    None
                }
            })
            .await;

        Ok(response)
    }

    async fn diagnostic(
        &self,
        params: lsp_types::DocumentDiagnosticParams,
    ) -> LspResult<lsp_types::DocumentDiagnosticReportResult> {
        tracing::debug!(
            "Received diagnostic request for {:?}",
            params.text_document.uri
        );

        let Some(url) =
            paths::parse_lsp_uri(&params.text_document.uri, paths::LspContext::Diagnostic)
        else {
            return Ok(lsp_types::DocumentDiagnosticReportResult::Report(
                lsp_types::DocumentDiagnosticReport::Full(
                    lsp_types::RelatedFullDocumentDiagnosticReport {
                        related_documents: None,
                        full_document_diagnostic_report: lsp_types::FullDocumentDiagnosticReport {
                            result_id: None,
                            items: vec![],
                        },
                    },
                ),
            ));
        };

        let path: Utf8PathBuf = url.path().into();

        // Only provide diagnostics for template files
        if FileKind::from(&path) != FileKind::Template {
            return Ok(lsp_types::DocumentDiagnosticReportResult::Report(
                lsp_types::DocumentDiagnosticReport::Full(
                    lsp_types::RelatedFullDocumentDiagnosticReport {
                        related_documents: None,
                        full_document_diagnostic_report: lsp_types::FullDocumentDiagnosticReport {
                            result_id: None,
                            items: vec![],
                        },
                    },
                ),
            ));
        }

        // Get diagnostics from the database
        let diagnostics: Vec<lsp_types::Diagnostic> = self
            .with_session_mut(|session| {
                let file = session.get_or_create_file(&path);
                session.with_db(|db| {
                    let nodelist = djls_templates::parse_template(db, file);
                    djls_ide::collect_diagnostics(db, file, nodelist)
                })
            })
            .await;

        Ok(lsp_types::DocumentDiagnosticReportResult::Report(
            lsp_types::DocumentDiagnosticReport::Full(
                lsp_types::RelatedFullDocumentDiagnosticReport {
                    related_documents: None,
                    full_document_diagnostic_report: lsp_types::FullDocumentDiagnosticReport {
                        result_id: None,
                        items: diagnostics,
                    },
                },
            ),
        ))
    }

    async fn goto_definition(
        &self,
        params: lsp_types::GotoDefinitionParams,
    ) -> LspResult<Option<lsp_types::GotoDefinitionResponse>> {
        let response = self
            .with_session_mut(|session| {
                let url = paths::parse_lsp_uri(
                    &params.text_document_position_params.text_document.uri,
                    paths::LspContext::GotoDefinition,
                )?;
                let path = paths::url_to_path(&url)?;
                let file = session.get_or_create_file(&path);

                session.with_db(|db| {
                    djls_ide::goto_template_definition(
                        db,
                        file,
                        params.text_document_position_params.position,
                        session.position_encoding(),
                    )
                })
            })
            .await;

        Ok(response)
    }

    async fn references(
        &self,
        params: lsp_types::ReferenceParams,
    ) -> LspResult<Option<Vec<lsp_types::Location>>> {
        let response = self
            .with_session_mut(|session| {
                let url = paths::parse_lsp_uri(
                    &params.text_document_position.text_document.uri,
                    paths::LspContext::References,
                )?;
                let path = paths::url_to_path(&url)?;
                let file = session.get_or_create_file(&path);

                session.with_db(|db| {
                    djls_ide::find_template_references(
                        db,
                        file,
                        params.text_document_position.position,
                        session.position_encoding(),
                    )
                })
            })
            .await;

        Ok(response)
    }

    async fn did_change_configuration(&self, _params: lsp_types::DidChangeConfigurationParams) {
        tracing::info!("Configuration change detected. Reloading settings...");

        self.with_session_mut(|session| {
            if session.project().is_some() {
                let project_root = session.database().project_root_or_cwd();

                match djls_conf::Settings::new(&project_root) {
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

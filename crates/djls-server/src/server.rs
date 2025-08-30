use std::future::Future;
use std::sync::Arc;

use djls_workspace::paths;
use tokio::sync::RwLock;
use tower_lsp_server::jsonrpc::Result as LspResult;
use tower_lsp_server::lsp_types;
use tower_lsp_server::LanguageServer;
use tracing_appender::non_blocking::WorkerGuard;
use url::Url;

use crate::queue::Queue;
use crate::session::Session;

const SERVER_NAME: &str = "Django Language Server";
const SERVER_VERSION: &str = "0.1.0";

pub struct DjangoLanguageServer {
    session: Arc<RwLock<Option<Session>>>,
    queue: Queue,
    _log_guard: WorkerGuard,
}

impl DjangoLanguageServer {
    #[must_use]
    pub fn new(log_guard: WorkerGuard) -> Self {
        Self {
            session: Arc::new(RwLock::new(None)),
            queue: Queue::new(),
            _log_guard: log_guard,
        }
    }

    pub async fn with_session<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Session) -> R,
        R: Default,
    {
        let session = self.session.read().await;
        if let Some(s) = &*session {
            f(s)
        } else {
            tracing::error!("Attempted to access session before initialization");
            R::default()
        }
    }

    pub async fn with_session_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut Session) -> R,
        R: Default,
    {
        let mut session = self.session.write().await;
        if let Some(s) = &mut *session {
            f(s)
        } else {
            tracing::error!("Attempted to access session before initialization");
            R::default()
        }
    }

    pub async fn with_session_task<F, Fut>(&self, f: F)
    where
        F: FnOnce(Arc<RwLock<Option<Session>>>) -> Fut + Send + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        let session_arc = Arc::clone(&self.session);

        if let Err(e) = self.queue.submit(async move { f(session_arc).await }).await {
            tracing::error!("Failed to submit task: {}", e);
        } else {
            tracing::info!("Task submitted successfully");
        }
    }
}

impl LanguageServer for DjangoLanguageServer {
    async fn initialize(
        &self,
        params: lsp_types::InitializeParams,
    ) -> LspResult<lsp_types::InitializeResult> {
        tracing::info!("Initializing server...");

        let session = Session::new(&params);

        {
            let mut session_lock = self.session.write().await;
            *session_lock = Some(session);
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
                ..Default::default()
            },
            server_info: Some(lsp_types::ServerInfo {
                name: SERVER_NAME.to_string(),
                version: Some(SERVER_VERSION.to_string()),
            }),
            offset_encoding: None,
        })
    }

    #[allow(clippy::too_many_lines)]
    async fn initialized(&self, _params: lsp_types::InitializedParams) {
        tracing::info!("Server received initialized notification.");

        self.with_session_task(|session_arc| async move {
            let project_path_and_venv = {
                let session_lock = session_arc.read().await;
                match &*session_lock {
                    Some(session) => session.project().map(|p| {
                        (
                            p.path().display().to_string(),
                            session
                                .settings()
                                .venv_path()
                                .map(std::string::ToString::to_string),
                        )
                    }),
                    None => None,
                }
            };

            if let Some((path_display, venv_path)) = project_path_and_venv {
                tracing::info!(
                    "Task: Starting initialization for project at: {}",
                    path_display
                );

                if let Some(ref path) = venv_path {
                    tracing::info!("Using virtual environment from config: {}", path);
                }

                let init_result = {
                    let mut session_lock = session_arc.write().await;
                    match &mut *session_lock {
                        Some(session) => {
                            if let Some(project) = session.project_mut().as_mut() {
                                project.initialize(venv_path.as_deref())
                            } else {
                                // Project was removed between read and write locks
                                Ok(())
                            }
                        }
                        None => Ok(()),
                    }
                };

                match init_result {
                    Ok(()) => {
                        tracing::info!("Task: Successfully initialized project: {}", path_display);
                    }
                    Err(e) => {
                        tracing::error!(
                            "Task: Failed to initialize Django project at {}: {}",
                            path_display,
                            e
                        );

                        // Clear project on error
                        let mut session_lock = session_arc.write().await;
                        if let Some(session) = &mut *session_lock {
                            *session.project_mut() = None;
                        }
                    }
                }
            } else {
                tracing::info!("Task: No project instance found to initialize.");
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

        self.with_session_mut(|session| {
            // Convert LSP types to our types
            let url =
                Url::parse(&params.text_document.uri.to_string()).expect("Valid URI from LSP");
            let language_id =
                djls_workspace::LanguageId::from(params.text_document.language_id.as_str());
            let document = djls_workspace::TextDocument::new(
                params.text_document.text,
                params.text_document.version,
                language_id,
            );

            session.open_document(&url, document);
        })
        .await;
    }

    async fn did_change(&self, params: lsp_types::DidChangeTextDocumentParams) {
        tracing::info!("Changed document: {:?}", params.text_document.uri);

        self.with_session_mut(|session| {
            let url =
                Url::parse(&params.text_document.uri.to_string()).expect("Valid URI from LSP");
            let new_version = params.text_document.version;
            let changes = params.content_changes;

            match session.apply_document_changes(&url, changes.clone(), new_version) {
                Ok(()) => {}
                Err(err) => {
                    tracing::warn!("{}", err);
                    // Recovery: handle full content changes only
                    if let Some(change) = changes.into_iter().next() {
                        let document = djls_workspace::TextDocument::new(
                            change.text,
                            new_version,
                            djls_workspace::LanguageId::Other,
                        );
                        session.update_document(&url, document);
                    }
                }
            }
        })
        .await;
    }

    async fn did_close(&self, params: lsp_types::DidCloseTextDocumentParams) {
        tracing::info!("Closed document: {:?}", params.text_document.uri);

        self.with_session_mut(|session| {
            let url =
                Url::parse(&params.text_document.uri.to_string()).expect("Valid URI from LSP");

            if session.close_document(&url).is_none() {
                tracing::warn!("Attempted to close document without overlay: {}", url);
            }
        })
        .await;
    }

    async fn completion(
        &self,
        params: lsp_types::CompletionParams,
    ) -> LspResult<Option<lsp_types::CompletionResponse>> {
        let response = self
            .with_session_mut(|session| {
                let lsp_uri = &params.text_document_position.text_document.uri;
                let url = Url::parse(&lsp_uri.to_string()).expect("Valid URI from LSP");
                let position = params.text_document_position.position;

                tracing::debug!("Completion requested for {} at {:?}", url, position);

                if let Some(path) = paths::url_to_path(&url) {
                    let content = session.file_content(path);
                    if content.is_empty() {
                        tracing::debug!("File {} has no content", url);
                    } else {
                        tracing::debug!("Using content for completion in {}", url);
                        // TODO: Implement actual completion logic using content
                    }
                }

                None
            })
            .await;

        Ok(response)
    }

    async fn did_change_configuration(&self, _params: lsp_types::DidChangeConfigurationParams) {
        tracing::info!("Configuration change detected. Reloading settings...");

        let project_path = self
            .with_session(|session| session.project().map(|p| p.path().to_path_buf()))
            .await;

        if let Some(path) = project_path {
            self.with_session_mut(|session| match djls_conf::Settings::new(path.as_path()) {
                Ok(new_settings) => {
                    session.set_settings(new_settings);
                }
                Err(e) => {
                    tracing::error!("Error loading settings: {}", e);
                }
            })
            .await;
        }
    }
}

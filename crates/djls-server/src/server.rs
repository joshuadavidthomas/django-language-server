use std::future::Future;
use std::sync::Arc;

use tokio::sync::RwLock;
use tower_lsp_server::jsonrpc::Result as LspResult;
use tower_lsp_server::lsp_types::CompletionOptions;
use tower_lsp_server::lsp_types::CompletionParams;
use tower_lsp_server::lsp_types::CompletionResponse;
use tower_lsp_server::lsp_types::DidChangeConfigurationParams;
use tower_lsp_server::lsp_types::DidChangeTextDocumentParams;
use tower_lsp_server::lsp_types::DidCloseTextDocumentParams;
use tower_lsp_server::lsp_types::DidOpenTextDocumentParams;
use tower_lsp_server::lsp_types::DidSaveTextDocumentParams;
use tower_lsp_server::lsp_types::InitializeParams;
use tower_lsp_server::lsp_types::InitializeResult;
use tower_lsp_server::lsp_types::InitializedParams;
use tower_lsp_server::lsp_types::OneOf;
use tower_lsp_server::lsp_types::SaveOptions;
use tower_lsp_server::lsp_types::ServerCapabilities;
use tower_lsp_server::lsp_types::ServerInfo;
use tower_lsp_server::lsp_types::TextDocumentSyncCapability;
use tower_lsp_server::lsp_types::TextDocumentSyncKind;
use tower_lsp_server::lsp_types::TextDocumentSyncOptions;
use tower_lsp_server::lsp_types::WorkspaceFoldersServerCapabilities;
use tower_lsp_server::lsp_types::WorkspaceServerCapabilities;
use tower_lsp_server::LanguageServer;
use tracing_appender::non_blocking::WorkerGuard;

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
    async fn initialize(&self, params: InitializeParams) -> LspResult<InitializeResult> {
        tracing::info!("Initializing server...");

        let session = Session::new(&params);

        {
            let mut session_lock = self.session.write().await;
            *session_lock = Some(session);
        }

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(false),
                    trigger_characters: Some(vec![
                        "{".to_string(),
                        "%".to_string(),
                        " ".to_string(),
                    ]),
                    ..Default::default()
                }),
                workspace: Some(WorkspaceServerCapabilities {
                    workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                        supported: Some(true),
                        change_notifications: Some(OneOf::Left(true)),
                    }),
                    file_operations: None,
                }),
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::INCREMENTAL),
                        will_save: Some(false),
                        will_save_wait_until: Some(false),
                        save: Some(SaveOptions::default().into()),
                    },
                )),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: SERVER_NAME.to_string(),
                version: Some(SERVER_VERSION.to_string()),
            }),
            offset_encoding: None,
        })
    }

    #[allow(clippy::too_many_lines)]
    async fn initialized(&self, _params: InitializedParams) {
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

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        tracing::info!("Opened document: {:?}", params.text_document.uri);

        self.with_session_mut(|session| {
            let db = session.db();
            session.documents_mut().handle_did_open(&db, &params);
        })
        .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        tracing::info!("Changed document: {:?}", params.text_document.uri);

        self.with_session_mut(|session| {
            let db = session.db();
            let _ = session.documents_mut().handle_did_change(&db, &params);
        })
        .await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        tracing::info!("Closed document: {:?}", params.text_document.uri);

        self.with_session_mut(|session| {
            session.documents_mut().handle_did_close(&params);
        })
        .await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        tracing::info!("Saved document: {:?}", params.text_document.uri);

        self.with_session_mut(|session| {
            if let Err(e) = session.documents_mut().handle_did_save(&params) {
                tracing::error!("Failed to handle did_save: {}", e);
            }
        })
        .await;
    }

    async fn completion(&self, params: CompletionParams) -> LspResult<Option<CompletionResponse>> {
        Ok(self
            .with_session(|session| {
                if let Some(project) = session.project() {
                    if let Some(tags) = project.template_tags() {
                        let db = session.db();
                        return session.documents().get_completions(
                            &db,
                            params.text_document_position.text_document.uri.as_str(),
                            params.text_document_position.position,
                            tags,
                        );
                    }
                }
                None
            })
            .await)
    }

    async fn did_change_configuration(&self, _params: DidChangeConfigurationParams) {
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

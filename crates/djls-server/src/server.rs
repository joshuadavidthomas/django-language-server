use std::sync::Arc;

use tokio::sync::RwLock;
use tower_lsp_server::jsonrpc::Result as LspResult;
use tower_lsp_server::lsp_types::{CompletionOptions, CompletionParams, CompletionResponse, DidChangeConfigurationParams, DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams, InitializeParams, InitializeResult, InitializedParams, MessageType, OneOf, SaveOptions, ServerCapabilities, ServerInfo, TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions, WorkspaceFoldersServerCapabilities, WorkspaceServerCapabilities};
use tower_lsp_server::Client;
use tower_lsp_server::LanguageServer;

use crate::queue::Queue;
use crate::session::Session;

const SERVER_NAME: &str = "Django Language Server";
const SERVER_VERSION: &str = "0.1.0";

pub struct DjangoLanguageServer {
    client: Client,
    session: Arc<RwLock<Session>>,
    queue: Queue,
}

impl DjangoLanguageServer {
    #[must_use] pub fn new(client: Client) -> Self {
        Self {
            client,
            session: Arc::new(RwLock::new(Session::default())),
            queue: Queue::new(),
        }
    }

    pub async fn with_session<R>(&self, f: impl FnOnce(&Session) -> R) -> R {
        let session = self.session.read().await;
        f(&session)
    }

    pub async fn with_session_mut<R>(&self, f: impl FnOnce(&mut Session) -> R) -> R {
        let mut session = self.session.write().await;
        f(&mut session)
    }
}

impl LanguageServer for DjangoLanguageServer {
    async fn initialize(&self, params: InitializeParams) -> LspResult<InitializeResult> {
        self.client
            .log_message(MessageType::INFO, "Initializing server...")
            .await;

        self.with_session_mut(|session| {
            *session.client_capabilities_mut() = Some(params.capabilities);
        })
        .await;

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
        self.client
            .log_message(
                MessageType::INFO,
                "Server received initialized notification.",
            )
            .await;

        let init_params = InitializeParams {
            // Using the current directory by default right now, but we should switch to
            // *falling back* to current dir if workspace folders is empty
            workspace_folders: None,
            ..Default::default()
        };

        let has_project =
            if let Some(project_path) = crate::workspace::get_project_path(&init_params) {
                self.with_session_mut(|session| {
                    let settings = djls_conf::Settings::new(&project_path)
                        .unwrap_or_else(|_| djls_conf::Settings::default());
                    *session.settings_mut() = settings;

                    *session.project_mut() = Some(djls_project::DjangoProject::new(project_path));
                    true
                })
                .await
            } else {
                false
            };

        if has_project {
            self.client
                .log_message(
                    MessageType::INFO,
                    "Project discovered from current directory",
                )
                .await;
        } else {
            self.client
                .log_message(
                    MessageType::INFO,
                    "No project discovered; running without project context",
                )
                .await;
        }

        let session_arc = Arc::clone(&self.session);
        let client = self.client.clone();

        if let Err(e) = self
            .queue
            .submit(async move {
                let project_path_and_venv = {
                    let session = session_arc.read().await;
                    session.project().map(|p| {
                        (
                            p.path().display().to_string(),
                            session.settings().venv_path().map(std::string::ToString::to_string),
                        )
                    })
                };

                if let Some((path_display, venv_path)) = project_path_and_venv {
                    client
                        .log_message(
                            MessageType::INFO,
                            &format!(
                                "Task: Starting initialization for project at: {path_display}"
                            ),
                        )
                        .await;

                    if let Some(ref path) = venv_path {
                        client
                            .log_message(
                                MessageType::INFO,
                                &format!("Using virtual environment from config: {path}"),
                            )
                            .await;
                    }

                    let init_result = {
                        let mut session = session_arc.write().await;
                        if let Some(project) = session.project_mut().as_mut() {
                            project.initialize(venv_path.as_deref())
                        } else {
                            // Project was removed between read and write locks
                            Ok(())
                        }
                    };

                    match init_result {
                        Ok(()) => {
                            client
                                .log_message(
                                    MessageType::INFO,
                                    &format!(
                                        "Task: Successfully initialized project: {path_display}"
                                    ),
                                )
                                .await;
                        }
                        Err(e) => {
                            client
                                .log_message(
                                    MessageType::ERROR,
                                    &format!(
                                        "Task: Failed to initialize Django project at {path_display}: {e}"
                                    ),
                                )
                                .await;

                            // Clear project on error
                            let mut session = session_arc.write().await;
                            *session.project_mut() = None;
                        }
                    }
                } else {
                    client
                        .log_message(
                            MessageType::INFO,
                            "Task: No project instance found to initialize.",
                        )
                        .await;
                }
                Ok(())
            })
            .await
        {
            self.client
                .log_message(
                    MessageType::ERROR,
                    &format!("Failed to submit project initialization task: {e}"),
                )
                .await;
        } else {
            self.client
                .log_message(MessageType::INFO, "Scheduled project initialization task.")
                .await;
        }
    }

    async fn shutdown(&self) -> LspResult<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        self.client
            .log_message(
                MessageType::INFO,
                &format!("Opened document: {:?}", params.text_document.uri),
            )
            .await;

        self.with_session_mut(|session| {
                let db = session.db_handle().db();
                session.documents_mut().handle_did_open(&db, &params);
            })
            .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        self.client
            .log_message(
                MessageType::INFO,
                &format!("Changed document: {:?}", params.text_document.uri),
            )
            .await;

        self.with_session_mut(|session| {
                let db = session.db_handle().db();
                let _ = session
                    .documents_mut()
                    .handle_did_change(&db, &params);
            })
            .await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        self.client
            .log_message(
                MessageType::INFO,
                &format!("Closed document: {:?}", params.text_document.uri),
            )
            .await;

        self.with_session_mut(|session| {
                session.documents_mut().handle_did_close(&params);
            })
            .await;
    }

    async fn completion(&self, params: CompletionParams) -> LspResult<Option<CompletionResponse>> {
        Ok(self
            .with_session(|session| {
                if let Some(project) = session.project() {
                    if let Some(tags) = project.template_tags() {
                        // Get a database instance from the handle
                        let db = session.db_handle().db();
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
        self.client
            .log_message(
                MessageType::INFO,
                "Configuration change detected. Reloading settings...",
            )
            .await;

        let project_path = self
            .with_session(|session| session.project().map(|p| p.path().to_path_buf()))
            .await;

        if let Some(path) = project_path {
            self.with_session_mut(|session| match djls_conf::Settings::new(path.as_path()) {
                Ok(new_settings) => {
                    *session.settings_mut() = new_settings;
                }
                Err(e) => {
                    eprintln!("Error loading settings: {e}");
                }
            })
            .await;
        }
    }
}

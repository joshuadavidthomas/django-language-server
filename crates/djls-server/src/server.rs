use crate::documents::Store;
use crate::queue::Queue;
use crate::workspace::get_project_path;
use djls_conf::Settings;
use djls_project::DjangoProject;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_lsp_server::jsonrpc::Result as LspResult;
use tower_lsp_server::lsp_types::*;
use tower_lsp_server::{Client, LanguageServer};

const SERVER_NAME: &str = "Django Language Server";
const SERVER_VERSION: &str = "0.1.0";

pub struct DjangoLanguageServer {
    client: Client,
    project: Arc<RwLock<Option<DjangoProject>>>,
    documents: Arc<RwLock<Store>>,
    settings: Arc<RwLock<Settings>>,
    queue: Queue,
}

impl DjangoLanguageServer {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            project: Arc::new(RwLock::new(None)),
            documents: Arc::new(RwLock::new(Store::new())),
            settings: Arc::new(RwLock::new(Settings::default())),
            queue: Queue::new(),
        }
    }

    async fn log_message(&self, type_: MessageType, message: &str) {
        self.client.log_message(type_, message).await;
    }

    async fn update_settings(&self, project_path: Option<&std::path::Path>) {
        if let Some(path) = project_path {
            match Settings::new(path) {
                Ok(loaded_settings) => {
                    let mut settings_guard = self.settings.write().await;
                    *settings_guard = loaded_settings;
                    // Could potentially check if settings actually changed before logging
                    self.log_message(
                        MessageType::INFO,
                        &format!(
                            "Successfully loaded/reloaded settings for {}",
                            path.display()
                        ),
                    )
                    .await;
                }
                Err(e) => {
                    // Keep existing settings if loading/reloading fails
                    self.log_message(
                        MessageType::ERROR,
                        &format!(
                            "Failed to load/reload settings for {}: {}",
                            path.display(),
                            e
                        ),
                    )
                    .await;
                }
            }
        } else {
            // If no project path, ensure we're using defaults (might already be the case)
            // Or log that project-specific settings can't be loaded.
            let mut settings_guard = self.settings.write().await;
            *settings_guard = Settings::default(); // Reset to default if no project path
            self.log_message(
                MessageType::INFO,
                "No project root identified. Using default settings.",
            )
            .await;
        }
    }
}

impl LanguageServer for DjangoLanguageServer {
    async fn initialize(&self, params: InitializeParams) -> LspResult<InitializeResult> {
        self.log_message(MessageType::INFO, "Initializing server...")
            .await;

        let project_path = get_project_path(&params);

        {
            // Scope for write lock
            let mut project_guard = self.project.write().await;
            if let Some(ref path) = project_path {
                self.log_message(
                    MessageType::INFO,
                    &format!(
                        "Project root identified: {}. Creating project instance.",
                        path.display()
                    ),
                )
                .await;
                *project_guard = Some(DjangoProject::new(path.clone()));
            } else {
                self.log_message(
                    MessageType::WARNING,
                    "Could not determine project root. Project features will be unavailable.",
                )
                .await;
                // Ensure it's None if no path
                *project_guard = None;
            }
        }

        self.update_settings(project_path.as_deref()).await;

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
                    // Add file operations if needed later
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

    async fn initialized(&self, _params: InitializedParams) {
        self.log_message(
            MessageType::INFO,
            "Server received initialized notification.",
        )
        .await;

        let project_arc = Arc::clone(&self.project);
        let client = self.client.clone();
        let settings_arc = Arc::clone(&self.settings);
        
        if let Err(e) = self.queue.submit(async move {
            let mut project_guard = project_arc.write().await;
            if let Some(project) = project_guard.as_mut() {
                let path_display = project.path().display().to_string();
                client
                    .log_message(
                        MessageType::INFO,
                        &format!(
                            "Task: Starting initialization for project at: {}",
                            path_display
                        ),
                    )
                    .await;

                let venv_path = {
                    let settings = settings_arc.read().await;
                    settings.venv_path().map(|s| s.to_string())
                };

                if let Some(ref path) = venv_path {
                    client
                        .log_message(
                            MessageType::INFO,
                            &format!("Using virtual environment from config: {}", path),
                        )
                        .await;
                }

                match project.initialize(venv_path.as_deref()) {
                    Ok(()) => {
                        client
                            .log_message(
                                MessageType::INFO,
                                &format!(
                                    "Task: Successfully initialized project: {}",
                                    path_display
                                ),
                            )
                            .await;
                    }
                    Err(e) => {
                        client
                            .log_message(
                                MessageType::ERROR,
                                &format!(
                                    "Task: Failed to initialize Django project at {}: {}",
                                    path_display, e
                                ),
                            )
                            .await;
                        *project_guard = None;
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
        }).await {
            self.log_message(
                MessageType::ERROR,
                &format!("Failed to submit project initialization task: {}", e),
            )
            .await;
        } else {
            self.log_message(MessageType::INFO, "Scheduled project initialization task.")
                .await;
        }
    }

    async fn shutdown(&self) -> LspResult<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        if let Err(e) = self.documents.write().await.handle_did_open(params.clone()) {
            eprintln!("Error handling document open: {}", e);
            return;
        }

        self.log_message(
            MessageType::INFO,
            &format!("Opened document: {:?}", params.text_document.uri),
        )
        .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        if let Err(e) = self
            .documents
            .write()
            .await
            .handle_did_change(params.clone())
        {
            eprintln!("Error handling document change: {}", e);
            return;
        }

        self.log_message(
            MessageType::INFO,
            &format!("Changed document: {:?}", params.text_document.uri),
        )
        .await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        if let Err(e) = self
            .documents
            .write()
            .await
            .handle_did_close(params.clone())
        {
            eprintln!("Error handling document close: {}", e);
            return;
        }

        self.log_message(
            MessageType::INFO,
            &format!("Closed document: {:?}", params.text_document.uri),
        )
        .await;
    }

    async fn completion(&self, params: CompletionParams) -> LspResult<Option<CompletionResponse>> {
        let project_guard = self.project.read().await;
        let documents_guard = self.documents.read().await;

        if let Some(project) = project_guard.as_ref() {
            if let Some(tags) = project.template_tags() {
                return Ok(documents_guard.get_completions(
                    params.text_document_position.text_document.uri.as_str(),
                    params.text_document_position.position,
                    tags,
                ));
            }
        }
        Ok(None)
    }

    async fn did_change_configuration(&self, _params: DidChangeConfigurationParams) {
        self.log_message(
            MessageType::INFO,
            "Configuration change detected. Reloading settings...",
        )
        .await;

        let project_path = {
            let project_guard = self.project.read().await;
            project_guard.as_ref().map(|p| p.path().to_path_buf())
        };

        self.update_settings(project_path.as_deref()).await;
    }
}

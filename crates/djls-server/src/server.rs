use crate::documents::Store;
use anyhow::Result;
use djls_project::DjangoProject;
use djls_worker::Worker;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result as LspResult;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

const SERVER_NAME: &str = "Django Language Server";
const SERVER_VERSION: &str = "0.1.0";

pub struct DjangoLanguageServer {
    client: Client,
    project: Arc<RwLock<Option<DjangoProject>>>,
    documents: Arc<RwLock<Store>>,
    worker: Worker,
}

impl DjangoLanguageServer {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            project: Arc::new(RwLock::new(None)),
            documents: Arc::new(RwLock::new(Store::new())),
            worker: Worker::new(),
        }
    }

    async fn log_message(&self, type_: MessageType, message: &str) -> Result<()> {
        self.client.log_message(type_, message).await;
        Ok(())
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for DjangoLanguageServer {
    async fn initialize(&self, params: InitializeParams) -> LspResult<InitializeResult> {
        let project = DjangoProject::from_initialize_params(&params);

        if let Some(mut project) = project {
            match project.initialize() {
                Ok(()) => {
                    self.log_message(
                        MessageType::INFO,
                        &format!("Using project path: {}", project.path().display()),
                    )
                    .await
                    .ok();
                    *self.project.write().await = Some(project);
                }
                Err(e) => {
                    self.log_message(
                        MessageType::ERROR,
                        &format!("Failed to initialize Django project: {}", e),
                    )
                    .await
                    .ok();
                }
            }
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

    async fn initialized(&self, _: InitializedParams) {
        self.log_message(MessageType::INFO, "server initialized!")
            .await
            .ok();
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
            &format!("Opened document: {}", params.text_document.uri),
        )
        .await
        .ok();
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
            &format!("Changed document: {}", params.text_document.uri),
        )
        .await
        .ok();
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
            &format!("Closed document: {}", params.text_document.uri),
        )
        .await
        .ok();
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
}

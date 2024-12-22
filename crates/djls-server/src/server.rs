use crate::documents::Store;
use crate::notifier::Notifier;
use crate::tasks::DebugTask;
use anyhow::Result;
use djls_ipc::{HealthCheck, HealthCheckRequestData, IpcClient};
use djls_worker::Worker;
use std::sync::Arc;
use std::time::Duration;
use tower_lsp::lsp_types::*;
use tracing::{debug, error, info, instrument, warn};

const SERVER_NAME: &str = "Django Language Server";
const SERVER_VERSION: &str = "0.1.0";

#[derive(Debug)]
pub enum LspRequest {
    Initialize(InitializeParams),
}

#[derive(Debug)]
pub enum LspNotification {
    DidOpenTextDocument(DidOpenTextDocumentParams),
    DidChangeTextDocument(DidChangeTextDocumentParams),
    DidCloseTextDocument(DidCloseTextDocumentParams),
    Initialized(InitializedParams),
    Shutdown,
}

pub struct DjangoLanguageServer {
    python: IpcClient,
    notifier: Arc<Box<dyn Notifier>>,
    documents: Store,
    worker: Worker,
}

impl DjangoLanguageServer {
    #[instrument(skip(python, notifier))]
    pub fn new(python: IpcClient, notifier: Box<dyn Notifier>) -> Self {
        debug!("Creating new DjangoLanguageServer instance");
        let notifier = Arc::new(notifier);

        Self {
            python,
            notifier,
            documents: Store::new(),
            worker: Worker::new(),
        }
    }

    #[instrument(skip(self))]
    pub async fn handle_request(&self, request: LspRequest) -> Result<InitializeResult> {
        debug!(?request, "Handling LSP request");
        match request {
            LspRequest::Initialize(_params) => {
                debug!("Initializing server capabilities");
                Ok(InitializeResult {
                    capabilities: ServerCapabilities {
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
                    offset_encoding: None,
                    server_info: Some(ServerInfo {
                        name: SERVER_NAME.to_string(),
                        version: Some(SERVER_VERSION.to_string()),
                    }),
                })
            }
        }
    }

    #[instrument(skip(self), err)]
    pub async fn handle_notification(&mut self, notification: LspNotification) -> Result<()> {
        debug!(?notification, "Handling LSP notification");

        match notification {
            LspNotification::DidOpenTextDocument(params) => {
                let uri = &params.text_document.uri;
                debug!(?uri, "Opening document");

                self.documents.handle_did_open(params.clone())?;
                info!(?uri, "Document opened successfully");

                self.notifier
                    .log_message(MessageType::INFO, &format!("Opened document: {}", uri))?;

                // Execute sync task
                debug!("Executing quick sync task");
                self.worker.execute(DebugTask::new(
                    "Quick task".to_string(),
                    Duration::from_millis(100),
                    self.notifier.clone(),
                ))?;

                // Submit async task
                debug!("Spawning important async task");
                let worker = self.worker.clone();
                let task = DebugTask::new(
                    "Important task".to_string(),
                    Duration::from_secs(1),
                    self.notifier.clone(),
                );
                tokio::spawn(async move {
                    if let Err(e) = worker.submit(task).await {
                        error!(?e, "Important task failed");
                    }
                });

                // Wait for result task
                debug!("Spawning task with result");
                let worker = self.worker.clone();
                let task = DebugTask::new(
                    "Task with result".to_string(),
                    Duration::from_secs(2),
                    self.notifier.clone(),
                );
                tokio::spawn(async move {
                    match worker.wait_for(task).await {
                        Ok(result) => debug!(?result, "Task completed successfully"),
                        Err(e) => error!(?e, "Task failed"),
                    }
                });

                Ok(())
            }
            LspNotification::DidChangeTextDocument(params) => {
                let uri = &params.text_document.uri;
                debug!(
                    ?uri,
                    changes = params.content_changes.len(),
                    "Changing document"
                );

                self.documents.handle_did_change(params.clone())?;
                debug!(?uri, "Document changed successfully");

                self.notifier
                    .log_message(MessageType::INFO, &format!("Changed document: {}", uri))?;
                Ok(())
            }
            LspNotification::DidCloseTextDocument(params) => {
                let uri = &params.text_document.uri;
                debug!(?uri, "Closing document");

                self.documents.handle_did_close(params.clone())?;
                info!(?uri, "Document closed successfully");

                self.notifier
                    .log_message(MessageType::INFO, &format!("Closed document: {}", uri))?;
                Ok(())
            }
            LspNotification::Initialized(_) => {
                info!("Server initialized, checking health");

                info!("CHECKPOINT 1: About to create request data");
                let request_data = HealthCheckRequestData {};
                info!("CHECKPOINT 2: Created request data");

                info!("CHECKPOINT 3: About to send health check");
                let send_future = self.python.send::<HealthCheck>(request_data);
                info!("CHECKPOINT 4: Created send future");

                info!("CHECKPOINT 5: About to await send future");
                match send_future.await {
                    Ok(health_check) => {
                        info!("CHECKPOINT 6: Got health check response");
                        info!(
                            status = %health_check.status,
                            version = %health_check.version,
                            "Health check completed"
                        );

                        info!("CHECKPOINT 7: About to send notification");
                        self.notifier.log_message(
                            MessageType::INFO,
                            &format!(
                                "Status: {}, Version: {}",
                                health_check.status, health_check.version
                            ),
                        )?;
                        info!("CHECKPOINT 8: Notification sent");
                        Ok(())
                    }
                    Err(e) => {
                        error!(?e, "CHECKPOINT ERROR: Health check failed");
                        Err(e.into())
                    }
                }
            }
            LspNotification::Shutdown => {
                info!("Server shutting down");
                Ok(())
            }
        }
    }
}

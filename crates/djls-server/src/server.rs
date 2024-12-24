use crate::documents::Store;
use crate::notifier::Notifier;
use crate::tasks::DebugTask;
use anyhow::Result;
use djls_project::DjangoProject;
use djls_worker::Worker;
use pyo3::prelude::*;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tower_lsp::lsp_types::*;

const SERVER_NAME: &str = "Django Language Server";
const SERVER_VERSION: &str = "0.1.0";

pub enum LspRequest {
    Initialize(InitializeParams),
    Completion(CompletionParams),
}

pub enum LspResponse {
    Initialize(InitializeResult),
    Completion(Option<CompletionResponse>),
}

pub enum LspNotification {
    DidOpenTextDocument(DidOpenTextDocumentParams),
    DidChangeTextDocument(DidChangeTextDocumentParams),
    DidCloseTextDocument(DidCloseTextDocumentParams),
    Initialized(InitializedParams),
    Shutdown,
}

pub struct DjangoLanguageServer {
    project: Option<DjangoProject>,
    notifier: Arc<Box<dyn Notifier>>,
    documents: Store,
    worker: Worker,
}

impl DjangoLanguageServer {
    pub fn new(notifier: Box<dyn Notifier>) -> Self {
        let notifier = Arc::new(notifier);

        Self {
            project: None,
            notifier,
            documents: Store::new(),
            worker: Worker::new(),
        }
    }

    pub fn handle_request(&mut self, request: LspRequest) -> Result<LspResponse> {
        match request {
            LspRequest::Initialize(params) => {
                if let Some(mut project) = DjangoProject::from_initialize_params(&params) {
                    if let Err(e) = project.initialize() {
                        self.notifier.log_message(
                            MessageType::ERROR,
                            &format!("Failed to initialize Django project: {}", e),
                        )?;
                    } else {
                        self.notifier.log_message(
                            MessageType::INFO,
                            &format!("Using project path: {}", project.path().display()),
                        )?;
                        self.project = Some(project);
                    }
                }

                Ok(LspResponse::Initialize(InitializeResult {
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
                    offset_encoding: None,
                    server_info: Some(ServerInfo {
                        name: SERVER_NAME.to_string(),
                        version: Some(SERVER_VERSION.to_string()),
                    }),
                }))
            }
            LspRequest::Completion(params) => {
                let completions = if let Some(project) = &self.project {
                    if let Some(tags) = project.template_tags() {
                        self.documents.get_completions(
                            params.text_document_position.text_document.uri.as_str(),
                            params.text_document_position.position,
                            tags,
                        )
                    } else {
                        None
                    }
                } else {
                    None
                };

                Ok(LspResponse::Completion(completions))
            }
        }
    }

    pub fn handle_notification(&mut self, notification: LspNotification) -> Result<()> {
        match notification {
            LspNotification::DidOpenTextDocument(params) => {
                self.documents.handle_did_open(params.clone())?;
                self.notifier.log_message(
                    MessageType::INFO,
                    &format!("Opened document: {}", params.text_document.uri),
                )?;
                Ok(())
            }
            LspNotification::DidChangeTextDocument(params) => {
                self.documents.handle_did_change(params.clone())?;
                self.notifier.log_message(
                    MessageType::INFO,
                    &format!("Changed document: {}", params.text_document.uri),
                )?;
                Ok(())
            }
            LspNotification::DidCloseTextDocument(params) => {
                self.documents.handle_did_close(params.clone())?;
                self.notifier.log_message(
                    MessageType::INFO,
                    &format!("Closed document: {}", params.text_document.uri),
                )?;
                Ok(())
            }
            LspNotification::Initialized(_) => {
                self.notifier
                    .log_message(MessageType::INFO, "server initialized!")?;
                Ok(())
            }
            LspNotification::Shutdown => Ok(()),
        }
    }
}

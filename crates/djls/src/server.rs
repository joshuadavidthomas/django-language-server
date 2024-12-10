use crate::documents::Store;
use crate::notifier::Notifier;
use anyhow::Result;
use djls_django::DjangoProject;
use tower_lsp::lsp_types::*;

const SERVER_NAME: &str = "Django Language Server";
const SERVER_VERSION: &str = "0.1.0";

pub enum LspRequest {
    Initialize(InitializeParams),
}

pub enum LspNotification {
    DidOpenTextDocument(DidOpenTextDocumentParams),
    DidChangeTextDocument(DidChangeTextDocumentParams),
    DidCloseTextDocument(DidCloseTextDocumentParams),
    Initialized(InitializedParams),
    Shutdown,
}

pub struct DjangoLanguageServer {
    django: DjangoProject,
    notifier: Box<dyn Notifier>,
    documents: Store,
}

impl DjangoLanguageServer {
    pub fn new(django: DjangoProject, notifier: Box<dyn Notifier>) -> Self {
        Self {
            django,
            notifier,
            documents: Store::new(),
        }
    }

    pub fn handle_request(&self, request: LspRequest) -> Result<InitializeResult> {
        match request {
            LspRequest::Initialize(_params) => Ok(InitializeResult {
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
            }),
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
                self.notifier
                    .log_message(MessageType::INFO, &format!("\n{}", self.django.py()))?;
                self.notifier
                    .log_message(MessageType::INFO, &format!("\n{}", self.django))?;
                Ok(())
            }
            LspNotification::Shutdown => Ok(()),
        }
    }
}

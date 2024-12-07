use crate::notifier::Notifier;
use anyhow::Result;
use djls_django::DjangoProject;
use tower_lsp::lsp_types::*;

pub enum LspRequest {
    Initialize(InitializeParams),
}

pub enum LspNotification {
    Initialized(InitializedParams),
    Shutdown,
}

pub struct DjangoLanguageServer {
    django: DjangoProject,
    notifier: Box<dyn Notifier>,
}

impl DjangoLanguageServer {
    pub fn new(django: DjangoProject, notifier: Box<dyn Notifier>) -> Self {
        Self { django, notifier }
    }

    pub fn handle_request(&self, request: LspRequest) -> Result<InitializeResult> {
        match request {
            LspRequest::Initialize(_params) => Ok(InitializeResult {
                capabilities: ServerCapabilities {
                    text_document_sync: Some(TextDocumentSyncCapability::Kind(
                        TextDocumentSyncKind::INCREMENTAL,
                    )),
                    ..Default::default()
                },
                offset_encoding: None,
                server_info: Some(ServerInfo {
                    name: String::from("Django Language Server"),
                    version: Some(String::from("0.1.0")),
                }),
            }),
        }
    }

    pub fn handle_notification(&self, notification: LspNotification) -> Result<()> {
        match notification {
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

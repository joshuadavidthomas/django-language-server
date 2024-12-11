mod documents;
mod notifier;
mod server;
mod tasks;

use crate::notifier::TowerLspNotifier;
use crate::server::{DjangoLanguageServer, LspNotification, LspRequest};
use anyhow::Result;
use djls_django::DjangoProject;
use djls_ipc::PythonProcess;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result as LspResult;
use tower_lsp::lsp_types::*;
use tower_lsp::{LanguageServer, LspService, Server};

struct TowerLspBackend {
    server: Arc<RwLock<DjangoLanguageServer>>,
}

#[tower_lsp::async_trait]
impl LanguageServer for TowerLspBackend {
    async fn initialize(&self, params: InitializeParams) -> LspResult<InitializeResult> {
        self.server
            .read()
            .await
            .handle_request(LspRequest::Initialize(params))
            .map_err(|_| tower_lsp::jsonrpc::Error::internal_error())
    }

    async fn initialized(&self, params: InitializedParams) {
        if let Err(e) = self
            .server
            .write()
            .await
            .handle_notification(LspNotification::Initialized(params))
        {
            eprintln!("Error handling initialized: {}", e);
        }
    }

    async fn shutdown(&self) -> LspResult<()> {
        self.server
            .write()
            .await
            .handle_notification(LspNotification::Shutdown)
            .map_err(|_| tower_lsp::jsonrpc::Error::internal_error())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        if let Err(e) = self
            .server
            .write()
            .await
            .handle_notification(LspNotification::DidOpenTextDocument(params))
        {
            eprintln!("Error handling document open: {}", e);
        }
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        if let Err(e) = self
            .server
            .write()
            .await
            .handle_notification(LspNotification::DidChangeTextDocument(params))
        {
            eprintln!("Error handling document change: {}", e);
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        if let Err(e) = self
            .server
            .write()
            .await
            .handle_notification(LspNotification::DidCloseTextDocument(params))
        {
            eprintln!("Error handling document close: {}", e);
        }
    }
}

pub async fn serve(python: PythonProcess) -> Result<()> {
    let django = DjangoProject::setup(python)?;

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::build(|client| {
        let notifier = Box::new(TowerLspNotifier::new(client.clone()));
        let server = DjangoLanguageServer::new(django, notifier);
        TowerLspBackend {
            server: Arc::new(RwLock::new(server)),
        }
    })
    .finish();

    Server::new(stdin, stdout, socket).serve(service).await;

    Ok(())
}

mod documents;
mod notifier;
mod server;
mod tasks;

use crate::notifier::TowerLspNotifier;
use crate::server::{DjangoLanguageServer, LspNotification, LspRequest};
use anyhow::Result;
use djls_ipc::IpcClient;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result as LspResult;
use tower_lsp::lsp_types::*;
use tower_lsp::{LanguageServer, LspService, Server};
use tracing::{debug, error, info, instrument};

pub struct TowerLspBackend {
    server: Arc<RwLock<DjangoLanguageServer>>,
}

#[tower_lsp::async_trait]
impl LanguageServer for TowerLspBackend {
    #[instrument(skip(self, params))]
    async fn initialize(&self, params: InitializeParams) -> LspResult<InitializeResult> {
        debug!("Acquiring read lock for initialize");
        let server = self.server.read().await;
        debug!("Acquired read lock");

        server
            .handle_request(LspRequest::Initialize(params))
            .await
            .map_err(|e| {
                error!(?e, "Initialize request failed");
                tower_lsp::jsonrpc::Error::internal_error()
            })
    }

    #[instrument(skip(self))]
    async fn initialized(&self, params: InitializedParams) {
        debug!("Acquiring write lock for initialized");
        let mut server = self.server.write().await;
        debug!("Acquired write lock");

        if let Err(e) = server
            .handle_notification(LspNotification::Initialized(params))
            .await
        {
            error!(?e, "Failed to handle initialized notification");
        }
    }

    #[instrument(skip(self))]
    async fn shutdown(&self) -> LspResult<()> {
        debug!("Acquiring write lock for shutdown");
        let mut server = self.server.write().await;
        debug!("Acquired write lock");

        server
            .handle_notification(LspNotification::Shutdown)
            .await
            .map_err(|e| {
                error!(?e, "Shutdown request failed");
                tower_lsp::jsonrpc::Error::internal_error()
            })
    }

    #[instrument(skip(self, params), fields(uri = ?params.text_document.uri))]
    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        debug!("Acquiring write lock for did_open");
        let mut server = self.server.write().await;
        debug!("Acquired write lock");

        if let Err(e) = server
            .handle_notification(LspNotification::DidOpenTextDocument(params))
            .await
        {
            error!(?e, "Failed to handle document open");
        }
    }

    #[instrument(skip(self, params), fields(uri = ?params.text_document.uri))]
    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        debug!("Acquiring write lock for did_change");
        let mut server = self.server.write().await;
        debug!("Acquired write lock");

        if let Err(e) = server
            .handle_notification(LspNotification::DidChangeTextDocument(params))
            .await
        {
            error!(?e, "Failed to handle document change");
        }
    }

    #[instrument(skip(self, params), fields(uri = ?params.text_document.uri))]
    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        debug!("Acquiring write lock for did_close");
        let mut server = self.server.write().await;
        debug!("Acquired write lock");

        if let Err(e) = server
            .handle_notification(LspNotification::DidCloseTextDocument(params))
            .await
        {
            error!(?e, "Failed to handle document close");
        }
    }
}

#[instrument]
pub async fn serve(python: IpcClient) -> Result<()> {
    info!("Starting LSP server");
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::build(|client| {
        debug!("Building LSP service");
        let notifier = Box::new(TowerLspNotifier::new(client.clone()));
        let server = DjangoLanguageServer::new(python, notifier);
        TowerLspBackend {
            server: Arc::new(RwLock::new(server)),
        }
    })
    .finish();

    debug!("Starting LSP server loop");
    Server::new(stdin, stdout, socket).serve(service).await;
    info!("LSP server shutdown");

    Ok(())
}

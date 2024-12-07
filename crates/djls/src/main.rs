mod notifier;
mod server;

use crate::notifier::TowerLspNotifier;
use crate::server::{DjangoLanguageServer, LspNotification, LspRequest};
use anyhow::Result;
use djls_django::DjangoProject;
use tower_lsp::jsonrpc::Result as LspResult;
use tower_lsp::lsp_types::*;
use tower_lsp::{LanguageServer, LspService, Server};

struct TowerLspBackend {
    server: DjangoLanguageServer,
}

#[tower_lsp::async_trait]
impl LanguageServer for TowerLspBackend {
    async fn initialize(&self, params: InitializeParams) -> LspResult<InitializeResult> {
        self.server
            .handle_request(LspRequest::Initialize(params))
            .map_err(|_| tower_lsp::jsonrpc::Error::internal_error())
    }

    async fn initialized(&self, params: InitializedParams) {
        if self
            .server
            .handle_notification(LspNotification::Initialized(params))
            .is_err()
        {
            // Handle error
        }
    }

    async fn shutdown(&self) -> LspResult<()> {
        self.server
            .handle_notification(LspNotification::Shutdown)
            .map_err(|_| tower_lsp::jsonrpc::Error::internal_error())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let django = DjangoProject::setup()?;

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::build(|client| {
        let notifier = Box::new(TowerLspNotifier::new(client.clone()));
        let server = DjangoLanguageServer::new(django, notifier);
        TowerLspBackend { server }
    })
    .finish();

    Server::new(stdin, stdout, socket).serve(service).await;

    Ok(())
}

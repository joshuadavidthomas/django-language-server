mod documents;
mod queue;
mod server;
mod workspace;

use crate::server::DjangoLanguageServer;
use anyhow::Result;
use tower_lsp_server::{LspService, Server};

pub async fn serve() -> Result<()> {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::build(DjangoLanguageServer::new).finish();

    Server::new(stdin, stdout, socket).serve(service).await;

    Ok(())
}

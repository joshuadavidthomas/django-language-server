mod documents;
mod server;
mod tasks;

use crate::server::DjangoLanguageServer;
use anyhow::Result;

pub async fn serve() -> Result<()> {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = tower_lsp::LspService::build(DjangoLanguageServer::new).finish();

    tower_lsp::Server::new(stdin, stdout, socket)
        .serve(service)
        .await;

    Ok(())
}

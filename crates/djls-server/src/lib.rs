mod client;
mod db;
mod documents;
mod queue;
mod server;
mod session;
mod workspace;

use anyhow::Result;
use tower_lsp_server::LspService;
use tower_lsp_server::Server;

use crate::server::DjangoLanguageServer;

pub fn run() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async {
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();

        let (service, socket) = LspService::build(|client| {
            // Initialize the global client
            client::init_client(client);
            // Then create the server instance
            DjangoLanguageServer::new()
        }).finish();

        Server::new(stdin, stdout, socket).serve(service).await;

        Ok(())
    })
}

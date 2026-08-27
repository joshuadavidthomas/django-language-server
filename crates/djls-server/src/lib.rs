#![cfg_attr(not(test), warn(clippy::expect_used))]

mod client;
mod document;
mod ext;
mod logging;
mod progress;
mod reload;
mod server;
mod session;
mod workspace;

use std::io::IsTerminal;

use anyhow::Result;
use tower_lsp_server::LspService;
use tower_lsp_server::Server;

use crate::server::DjangoLanguageServer;

/// Run the Django language server.
pub fn run() -> Result<()> {
    if std::io::stdin().is_terminal() {
        eprintln!("`djls serve` communicates with an editor over standard input and output.");
        eprintln!("No editor is connected; waiting for LSP messages.");
        eprintln!("Press Enter to stop the server. Ctrl+C may not work with LSP stdio.");
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async {
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();

        let (service, socket) = LspService::build(|client| {
            let logging = logging::init_tracing({
                let client = client.clone();
                move |message_type, message| {
                    let client = client.clone();
                    tokio::spawn(async move {
                        client.log_message(message_type, message).await;
                    });
                }
            });

            DjangoLanguageServer::new(client, logging)
        })
        .finish();

        Server::new(stdin, stdout, socket).serve(service).await;

        Ok(())
    })
}

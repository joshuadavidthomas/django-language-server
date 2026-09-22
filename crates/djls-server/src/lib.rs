#![cfg_attr(not(test), warn(clippy::expect_used))]

mod client;
mod diagnostics;
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
        eprintln!("Django Language Server is running directly in a terminal.");
        eprintln!(
            "This server is designed to communicate over stdin/stdout with a language client."
        );
        eprintln!("It is not intended to be used directly in a terminal.");
        eprintln!();
        eprintln!("The server is now waiting for LSP messages, but no editor is connected.");
        eprintln!("To exit: press ENTER to send invalid input and trigger an error exit.");
        eprintln!("Ctrl+C may not work as expected due to LSP stdio communication.");
    }

    // Locals drop in reverse order: flush file tracing only after runtime teardown.
    let logging = logging::init_tracing();
    let lsp_logging = logging.lsp();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async {
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();

        let (service, socket) = LspService::build(|client| {
            lsp_logging.start(client.clone());
            DjangoLanguageServer::new(client, lsp_logging.clone())
        })
        .finish();

        Server::new(stdin, stdout, socket).serve(service).await;
        lsp_logging.stop().await;

        Ok(())
    })
}

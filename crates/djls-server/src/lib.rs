pub mod db;
mod encoding;
mod logging;
mod queue;
pub mod server;
pub mod session;

use std::io::IsTerminal;

use anyhow::Result;
use tower_lsp_server::LspService;
use tower_lsp_server::Server;

pub use crate::server::DjangoLanguageServer;
pub use crate::session::Session;

pub fn run() -> Result<()> {
    if std::io::stdin().is_terminal() {
        eprintln!(
            "---------------------------------------------------------------------------------"
        );
        eprintln!("Django Language Server is running directly in a terminal.");
        eprintln!(
            "This server is designed to communicate over stdin/stdout with a language client."
        );
        eprintln!("It is not intended to be used directly in a terminal.");
        eprintln!();
        eprintln!(
            "Note: The server is now waiting for LSP messages, but since you're in a terminal,"
        );
        eprintln!("no editor is connected and the server won't do anything.");
        eprintln!();
        eprintln!("To exit: Press ENTER to send invalid input and trigger an error exit.");
        eprintln!("Ctrl+C will not work as expected due to LSP stdio communication.");
        eprintln!(
            "---------------------------------------------------------------------------------"
        );
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async {
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();

        let (service, socket) = LspService::build(|client| {
            let log_guard = logging::init_tracing({
                let client = client.clone();
                move |message_type, message| {
                    let client = client.clone();
                    tokio::spawn(async move {
                        client.log_message(message_type, message).await;
                    });
                }
            });

            DjangoLanguageServer::new(client, log_guard)
        })
        .finish();

        Server::new(stdin, stdout, socket).serve(service).await;

        Ok(())
    })
}

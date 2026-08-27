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
use std::net::SocketAddr;

use anyhow::Context as _;
use anyhow::Result;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::net::TcpListener;
use tower_lsp_server::LspService;
use tower_lsp_server::Server;

use crate::server::DjangoLanguageServer;

/// Transport used by the language server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Connection {
    /// Communicate over standard input and output.
    Stdio,
    /// Listen for one client at the given address.
    Tcp(SocketAddr),
}

/// Run the Django language server.
pub fn run(connection: Connection) -> Result<()> {
    if connection == Connection::Stdio && std::io::stdin().is_terminal() {
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

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async move {
        match connection {
            Connection::Stdio => serve(tokio::io::stdin(), tokio::io::stdout()).await,
            Connection::Tcp(address) => {
                let listener = TcpListener::bind(address)
                    .await
                    .with_context(|| format!("Failed to bind TCP listener at {address}"))?;
                let local_address = listener
                    .local_addr()
                    .context("Failed to read TCP listener address")?;
                eprintln!("Listening for an LSP client at {local_address}");
                let (stream, _) = listener
                    .accept()
                    .await
                    .context("Failed to accept TCP client")?;
                let (reader, writer) = tokio::io::split(stream);
                serve(reader, writer).await
            }
        }
    })
}

async fn serve<R, W>(reader: R, writer: W) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite,
{
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

    Server::new(reader, writer, socket).serve(service).await;
    Ok(())
}

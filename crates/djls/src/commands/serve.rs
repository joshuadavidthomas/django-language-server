use crate::args::Args;
use crate::commands::Command;
use crate::exit::Exit;
use anyhow::Result;
use clap::{Parser, ValueEnum};
use djls_server::DjangoLanguageServer;
use tower_lsp_server::{LspService, Server};

#[derive(Debug, Parser)]
pub struct Serve {
    #[arg(short, long, default_value_t = ConnectionType::Stdio, value_enum)]
    connection_type: ConnectionType,
}

#[derive(Clone, Debug, ValueEnum)]
enum ConnectionType {
    Stdio,
    Tcp,
}

impl Command for Serve {
    fn execute(&self, _args: &Args) -> Result<Exit> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();

        let exit_status = runtime.block_on(async {
            let stdin = tokio::io::stdin();
            let stdout = tokio::io::stdout();

            let (service, socket) = LspService::build(DjangoLanguageServer::new).finish();

            Server::new(stdin, stdout, socket).serve(service).await;

            // Exit here instead of returning control to the `Cli`, for ... reasons?
            // If we don't exit here, ~~~ something ~~~ goes on with PyO3 (I assume)
            // or the Python entrypoint wrapper to indefinitely hang the CLI and keep
            // the process running
            Exit::success()
                .with_message("Server completed successfully")
                .process_exit()
        });

        Ok(exit_status)
    }
}

use anyhow::Result;
use clap::Parser;
use clap::ValueEnum;

use crate::args::Args;
use crate::commands::Command;
use crate::exit::Exit;

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
        djls_server::run()?;

        // Exit here instead of returning control to the `Cli`, for ... reasons?
        // If we don't exit here, ~~~ something ~~~ goes on with PyO3 (I assume)
        // or the Python entrypoint wrapper to indefinitely hang the CLI and keep
        // the process running
        Exit::success()
            .with_message("Server completed successfully")
            .process_exit()
    }
}

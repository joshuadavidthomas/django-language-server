mod commands;

use crate::commands::Serve;
use anyhow::Result;
use clap::{Parser, Subcommand};
use djls_ipc::{JsonProtocol, Protocol, PythonProcess};
use std::ffi::OsStr;
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "djls")]
#[command(version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,

    #[command(flatten)]
    args: Args,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start the LSP server
    Serve(Serve),
}

#[derive(Parser)]
pub struct Args {
    #[command(flatten)]
    global: GlobalArgs,
}

#[derive(Parser, Debug, Clone)]
struct GlobalArgs {
    /// Do not print any output.
    #[arg(global = true, long, short, conflicts_with = "verbose")]
    pub quiet: bool,

    /// Use verbose output.
    #[arg(global = true, action = clap::ArgAction::Count, long, short, conflicts_with = "quiet")]
    pub verbose: u8,
}

#[tokio::main]
async fn main() -> Result<ExitCode> {
    let cli = Cli::parse();
    match cli.command {
        Command::Serve(_serve) => {
            let python =
                PythonProcess::new::<Vec<&OsStr>, &OsStr>("djls_agent", None, JsonProtocol)?;
            djls_server::serve(python).await?
        }
    }
    Ok(ExitCode::SUCCESS)
}

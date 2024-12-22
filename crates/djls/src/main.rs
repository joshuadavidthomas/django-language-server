mod commands;

use crate::commands::Serve;
use anyhow::Result;
use clap::{Parser, Subcommand};
use djls_ipc::IpcClient;
use std::process::ExitCode;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{fmt, EnvFilter};

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

    let filter = match (cli.args.global.quiet, cli.args.global.verbose) {
        (true, _) => EnvFilter::new("error"),
        (false, 0) => EnvFilter::new("info"),
        (false, 1) => EnvFilter::new("debug"),
        (false, _) => EnvFilter::new("trace"),
    };

    let file_appender = RollingFileAppender::new(
        Rotation::DAILY,
        "logs",     // directory
        "djls.log", // file name
    );

    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true)
        .with_writer(non_blocking)
        .init();

    match cli.command {
        Command::Serve(_serve) => {
            tracing::info!("Starting LSP server");
            let client = IpcClient::new("djls_agent").await?;
            djls_server::serve(client).await?
        }
    }
    Ok(ExitCode::SUCCESS)
}

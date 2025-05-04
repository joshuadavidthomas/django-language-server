use crate::args::Args;
use crate::commands::{Command, DjlsCommand};
use anyhow::Result;
use clap::Parser;
use std::process::ExitCode;

/// The main CLI structure that defines the command-line interface
#[derive(Parser)]
#[command(name = "djls")]
#[command(version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: DjlsCommand,

    #[command(flatten)]
    pub args: Args,
}

/// Parse CLI arguments and execute the chosen command
pub async fn run(args: Vec<String>) -> Result<ExitCode> {
    let cli = Cli::try_parse_from(args).unwrap_or_else(|e| {
        e.exit();
    });

    cli.command.execute(&cli.args.global).await
}
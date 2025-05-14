use std::fmt::Write;

use anyhow::Result;
use clap::Parser;

use crate::args::Args;
use crate::commands::Command;
use crate::commands::DjlsCommand;
use crate::exit::Exit;

/// Main CLI structure that defines the command-line interface
#[derive(Parser)]
#[command(name = "djls")]
#[command(version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: DjlsCommand,

    #[command(flatten)]
    pub args: Args,
}

/// Parse CLI arguments, execute the chosen command, and handle results
pub fn run(args: Vec<String>) -> Result<()> {
    let cli = Cli::try_parse_from(args).unwrap_or_else(|e| {
        e.exit();
    });

    let result = match &cli.command {
        DjlsCommand::Serve(cmd) => cmd.execute(&cli.args),
    };

    match result {
        Ok(exit) => exit.process_exit(),
        Err(e) => {
            let mut msg = e.to_string();
            if let Some(source) = e.source() {
                let _ = write!(msg, ", caused by {source}");
            }
            Exit::error().with_message(msg).process_exit()
        }
    }
}

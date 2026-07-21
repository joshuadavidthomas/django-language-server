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
pub(crate) struct Cli {
    #[command(subcommand)]
    command: DjlsCommand,

    #[command(flatten)]
    args: Args,
}

/// Parse CLI arguments, execute the chosen command, and handle results
pub(crate) fn run(args: Vec<String>) -> Result<()> {
    let cli = Cli::try_parse_from(args).unwrap_or_else(|e| {
        e.exit();
    });

    let result = match &cli.command {
        DjlsCommand::Check(cmd) => cmd.execute(&cli.args),
        DjlsCommand::Serve(cmd) => cmd.execute(&cli.args),
    };

    match result {
        Ok(exit) => exit.process_exit(),
        Err(e) => {
            let mut msg = e.to_string();
            if let Some(source) = e.source() {
                msg.push_str(", caused by ");
                msg.push_str(&source.to_string());
            }
            Exit::error().with_message(msg).process_exit()
        }
    }
}

pub mod serve;

use crate::args::Args;
use anyhow::Result;
use clap::Subcommand;
use std::process::ExitCode;

pub trait Command {
    async fn execute(&self, args: &Args) -> Result<ExitCode>;
}

#[derive(Debug, Subcommand)]
pub enum DjlsCommand {
    /// Start the LSP server
    Serve(self::serve::Serve),
}

impl Command for DjlsCommand {
    async fn execute(&self, args: &Args) -> Result<ExitCode> {
        match self {
            DjlsCommand::Serve(cmd) => cmd.execute(args).await,
        }
    }
}

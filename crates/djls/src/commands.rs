pub mod serve;

use crate::args::GlobalArgs;
use anyhow::Result;
use clap::Subcommand;
use std::process::ExitCode;

pub trait Command {
    async fn execute(&self, global_args: &GlobalArgs) -> Result<ExitCode>;
}

#[derive(Debug, Subcommand)]
pub enum DjlsCommand {
    /// Start the LSP server
    Serve(self::serve::Serve),
}

impl Command for DjlsCommand {
    async fn execute(&self, global_args: &GlobalArgs) -> Result<ExitCode> {
        match self {
            DjlsCommand::Serve(cmd) => cmd.execute(global_args).await,
        }
    }
}

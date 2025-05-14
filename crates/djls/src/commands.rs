mod serve;

use anyhow::Result;
use clap::Subcommand;

use crate::args::Args;
use crate::exit::Exit;

pub trait Command {
    fn execute(&self, args: &Args) -> Result<Exit>;
}

#[derive(Debug, Subcommand)]
pub enum DjlsCommand {
    /// Start the LSP server
    Serve(self::serve::Serve),
}

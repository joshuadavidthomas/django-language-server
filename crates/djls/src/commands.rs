pub mod check;
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
    /// Check Django template files for errors
    Check(self::check::Check),
    /// Start the LSP server
    Serve(self::serve::Serve),
}

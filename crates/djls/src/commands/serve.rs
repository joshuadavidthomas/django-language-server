use anyhow::bail;
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

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ConnectionType {
    Stdio,
    Tcp,
}

impl Command for Serve {
    fn execute(&self, _args: &Args) -> Result<Exit> {
        match self.connection_type {
            ConnectionType::Stdio => {
                djls_server::run()?;
                Ok(Exit::success())
            }
            ConnectionType::Tcp => bail!("`djls serve --connection-type tcp` is not supported yet"),
        }
    }
}

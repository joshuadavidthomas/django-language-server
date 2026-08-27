use std::net::SocketAddr;

use anyhow::Result;
use anyhow::bail;
use clap::Parser;
use clap::ValueEnum;

use crate::args::Args;
use crate::commands::Command;
use crate::exit::Exit;

#[derive(Debug, Parser)]
pub(crate) struct Serve {
    /// How the language server communicates with its client.
    #[arg(short, long, default_value_t = ConnectionType::Stdio, value_enum)]
    connection_type: ConnectionType,

    /// IP address and port to listen on for TCP connections.
    #[arg(long, required_if_eq("connection_type", "tcp"))]
    address: Option<SocketAddr>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ConnectionType {
    Stdio,
    Tcp,
}

impl Command for Serve {
    fn execute(&self, _args: &Args) -> Result<Exit> {
        let connection = match (self.connection_type, self.address) {
            (ConnectionType::Stdio, None) => djls_server::Connection::Stdio,
            (ConnectionType::Stdio, Some(_)) => {
                bail!("`--address` can only be used with `--connection-type tcp`");
            }
            (ConnectionType::Tcp, Some(address)) => djls_server::Connection::Tcp(address),
            (ConnectionType::Tcp, None) => {
                bail!("`--address` is required with `--connection-type tcp`");
            }
        };
        djls_server::run(connection)?;
        Ok(Exit::success())
    }
}

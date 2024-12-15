use clap::{Parser, ValueEnum};

#[derive(Debug, Parser)]
pub struct Serve {
    #[arg(short, long, default_value_t = ConnectionType::Stdio, value_enum)]
    connection_type: ConnectionType,
}

#[derive(Clone, Debug, ValueEnum)]
enum ConnectionType {
    Stdio,
    Tcp,
}

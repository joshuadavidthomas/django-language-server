use crate::args::GlobalArgs;
use crate::commands::Command;
use anyhow::Result;
use clap::{Parser, ValueEnum};
use std::process::ExitCode;

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

impl Command for Serve {
    async fn execute(&self, _global_args: &GlobalArgs) -> Result<ExitCode> {
        // You can use global_args here to adjust behavior
        // For example: if global_args.verbose > 0 { println!("Starting server..."); }
        djls_server::serve().await?;
        Ok(ExitCode::SUCCESS)
    }
}


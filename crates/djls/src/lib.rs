mod commands;

use crate::commands::Serve;
use anyhow::Result;
use clap::{Parser, Subcommand};
use pyo3::prelude::*;
use std::env;
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "djls")]
#[command(version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,

    #[command(flatten)]
    args: Args,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start the LSP server
    Serve(Serve),
}

#[derive(Parser)]
pub struct Args {
    #[command(flatten)]
    global: GlobalArgs,
}

#[derive(Parser, Debug, Clone)]
struct GlobalArgs {
    /// Do not print any output.
    #[arg(global = true, long, short, conflicts_with = "verbose")]
    pub quiet: bool,

    /// Use verbose output.
    #[arg(global = true, action = clap::ArgAction::Count, long, short, conflicts_with = "quiet")]
    pub verbose: u8,
}

#[pyfunction]
fn entrypoint(_py: Python) -> PyResult<()> {
    // Skip python interpreter and script path, add command name
    let args: Vec<String> = std::iter::once("djls".to_string())
        .chain(env::args().skip(2))
        .collect();

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, async move {
        tokio::select! {
            // The main CLI program
            result = main(args) => {
                match result {
                    Ok(code) => {
                        if code != ExitCode::SUCCESS {
                            std::process::exit(1);
                        }
                        Ok::<(), PyErr>(())
                    }
                    Err(e) => {
                        eprintln!("Error: {}", e);
                        if let Some(source) = e.source() {
                            eprintln!("Caused by: {}", source);
                        }
                        std::process::exit(1);
                    }
                }
            }
            // Ctrl+C handling
            _ = tokio::signal::ctrl_c() => {
                println!("\nReceived Ctrl+C, shutting down...");
                // Cleanup code here if needed
                std::process::exit(130); // Standard Ctrl+C exit code
            }
            // SIGTERM handling (Unix only)
            _ = async {
                #[cfg(unix)]
                {
                    use tokio::signal::unix::{signal, SignalKind};
                    let mut term = signal(SignalKind::terminate()).unwrap();
                    term.recv().await;
                }
            } => {
                println!("\nReceived termination signal, shutting down...");
                std::process::exit(143); // Standard SIGTERM exit code
            }
        }
    })?;

    Ok(())
}

async fn main(args: Vec<String>) -> Result<ExitCode> {
    let cli = Cli::try_parse_from(args).unwrap_or_else(|e| {
        e.exit();
    });

    match cli.command {
        Command::Serve(_serve) => djls_server::serve().await?,
    }

    Ok(ExitCode::SUCCESS)
}

#[pymodule]
fn djls(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(entrypoint, m)?)?;
    Ok(())
}

mod commands;

use crate::commands::Serve;
use anyhow::Result;
use clap::{Parser, Subcommand};
use pyo3;
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

async fn run() -> Result<ExitCode> {
    let cli = Cli::parse();

    match cli.command {
        Command::Serve(_serve) => djls_server::serve().await?,
    }

    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    // Initialize Python interpreter
    pyo3::prepare_freethreaded_python();

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let local = tokio::task::LocalSet::new();
    let exit_code = local.block_on(&runtime, async move {
        tokio::select! {
            // The main CLI program
            result = run() => {
                match result {
                    Ok(code) => code,
                    Err(e) => {
                        eprintln!("Error: {}", e);
                        if let Some(source) = e.source() {
                            eprintln!("Caused by: {}", source);
                        }
                        ExitCode::FAILURE
                    }
                }
            }
            // Ctrl+C handling
            _ = tokio::signal::ctrl_c() => {
                println!("\nReceived Ctrl+C, shutting down...");
                // Cleanup code here if needed
                ExitCode::from(130) // Standard Ctrl+C exit code
            }
            // SIGTERM handling (Unix only)
            _ = async {
                #[cfg(unix)]
                {
                    use tokio::signal::unix::{signal, SignalKind};
                    let mut term = signal(SignalKind::terminate()).unwrap();
                    term.recv().await;
                }
                #[cfg(not(unix))]
                {
                    // On non-unix platforms, this future never completes
                    std::future::pending::<()>().await;
                }
            } => {
                println!("\nReceived termination signal, shutting down...");
                ExitCode::from(143) // Standard SIGTERM exit code
            }
        }
    });

    exit_code
}
use std::fmt::Write;

use anyhow::Result;
use clap::Parser;
use directories::ProjectDirs;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;

use crate::args::Args;
use crate::commands::Command;
use crate::commands::DjlsCommand;
use crate::exit::Exit;

/// Main CLI structure that defines the command-line interface
#[derive(Parser)]
#[command(name = "djls")]
#[command(version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: DjlsCommand,

    #[command(flatten)]
    pub args: Args,
}

/// Initialize tracing with file logging to XDG directories
fn initialize_tracing() -> Result<tracing_appender::non_blocking::WorkerGuard> {
    // Get XDG-compliant log directory
    let log_dir = if let Some(proj_dirs) = ProjectDirs::from("com", "djls", "django-language-server") {
        proj_dirs.cache_dir().to_path_buf()
    } else {
        // Fallback to current directory if XDG not available
        std::env::current_dir()?.join("logs")
    };

    // Create log directory if it doesn't exist
    std::fs::create_dir_all(&log_dir)?;

    // Set up rolling file appender (daily rotation)
    let file_appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("djls")
        .filename_suffix("log")
        .build(&log_dir)?;

    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    // Initialize tracing subscriber with env filter
    tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("djls=debug".parse()?)
                .add_directive("djls_server=debug".parse()?)
                .add_directive("djls_project=info".parse()?)
        )
        .with_ansi(false) // Disable ANSI colors for file output
        .init();

    tracing::info!("Tracing initialized, logs writing to: {}", log_dir.display());
    
    Ok(guard)
}

/// Parse CLI arguments, execute the chosen command, and handle results
pub fn run(args: Vec<String>) -> Result<()> {
    // Initialize tracing first
    let _guard = initialize_tracing()?;
    
    tracing::info!("Django Language Server starting");
    tracing::debug!("CLI args: {:?}", args);

    let cli = Cli::try_parse_from(args).unwrap_or_else(|e| {
        tracing::error!("Failed to parse CLI arguments: {}", e);
        e.exit();
    });

    let result = match &cli.command {
        DjlsCommand::Serve(cmd) => {
            tracing::info!("Starting LSP server");
            cmd.execute(&cli.args)
        }
    };

    match result {
        Ok(exit) => {
            tracing::info!("Command completed successfully");
            exit.process_exit()
        }
        Err(e) => {
            let mut msg = e.to_string();
            if let Some(source) = e.source() {
                let _ = write!(msg, ", caused by {source}");
            }
            tracing::error!("Command failed: {}", msg);
            Exit::error().with_message(msg).process_exit()
        }
    }
}

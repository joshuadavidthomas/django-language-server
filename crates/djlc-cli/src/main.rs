use clap::{Args, Parser, Subcommand};
use djls_ipc::{PythonProcess, Transport};
use std::time::Duration;

#[derive(Debug, Parser)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Args)]
struct CommonOpts {
    /// Disable periodic health checks
    #[arg(long)]
    no_health_check: bool,

    /// Health check interval in seconds
    #[arg(long, default_value = "30")]
    health_interval: u64,
}

impl CommonOpts {
    fn health_check_interval(&self) -> Option<Duration> {
        if self.no_health_check {
            None
        } else {
            Some(Duration::from_secs(self.health_interval))
        }
    }
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Start the LSP server
    Serve(CommonOpts),
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Serve(opts) => {
            let python =
                PythonProcess::new("djls.lsp", Transport::Json, opts.health_check_interval())?;
            djls_server::serve(python).await?
        }
    }

    Ok(())
}

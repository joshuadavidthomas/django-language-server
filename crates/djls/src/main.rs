/// Binary interface for local development only.
///
/// This binary exists for development and testing with `cargo run`.
/// The production CLI is distributed through the `PyO3` interface in lib.rs.
mod args;
mod cli;
mod commands;
mod exit;

use anyhow::Result;

/// Process CLI args and run the appropriate command.
fn main() -> Result<()> {
    // Get command line arguments
    let args: Vec<String> = std::env::args().collect();

    // Call the unified CLI run function
    cli::run(args)
}

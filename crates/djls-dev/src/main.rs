mod generate_rules;

use anyhow::Result;
use clap::Parser;
use clap::Subcommand;

#[derive(Parser)]
#[command(
    name = "djls-dev",
    about = "Development tools for django-language-server"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generate docs/rules.md from rule definitions.
    GenerateRules,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::GenerateRules => generate_rules::run()?,
    }

    Ok(())
}

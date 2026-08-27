use anyhow::Result;
use clap::Parser;

use crate::args::Args;
use crate::commands::Command;
use crate::commands::DjlsCommand;
use crate::exit::Exit;

/// Main CLI structure that defines the command-line interface
#[derive(Parser)]
#[command(name = "djls")]
#[command(version, about)]
pub(crate) struct Cli {
    #[command(subcommand)]
    command: DjlsCommand,

    #[command(flatten)]
    args: Args,
}

/// Parse CLI arguments, execute the chosen command, and handle results
pub(crate) fn run(args: Vec<String>) -> Result<()> {
    let cli = Cli::try_parse_from(args).unwrap_or_else(|e| {
        e.exit();
    });

    let result = match &cli.command {
        DjlsCommand::Check(cmd) => cmd.execute(&cli.args),
        DjlsCommand::Serve(cmd) => cmd.execute(&cli.args),
    };

    match result {
        Ok(exit) => exit.process_exit(),
        Err(error) => Exit::error()
            .with_message(format_error(&error))
            .process_exit(),
    }
}

fn format_error(error: &anyhow::Error) -> String {
    let mut message = format!("error: {error}");
    for cause in error.chain().skip(1) {
        message.push_str("\n  caused by: ");
        message.push_str(&cause.to_string());
    }
    message
}

#[cfg(test)]
mod tests {
    use anyhow::Context as _;

    use super::format_error;

    #[test]
    fn formats_the_complete_error_chain() {
        let error = Err::<(), _>(anyhow::anyhow!("root cause"))
            .context("middle context")
            .context("top context")
            .expect_err("test error chain should fail");

        assert_eq!(
            format_error(&error),
            "error: top context\n  caused by: middle context\n  caused by: root cause"
        );
    }
}

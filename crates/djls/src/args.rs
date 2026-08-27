use clap::Parser;
use djls_server::LogVerbosity;

#[derive(Parser, Debug, Clone)]
pub(crate) struct Args {
    /// Do not print any output.
    #[arg(global = true, long, short, conflicts_with = "verbose")]
    pub quiet: bool,

    /// Increase log verbosity. Use `-v` for debug and `-vv` for trace.
    #[arg(global = true, action = clap::ArgAction::Count, long, short, conflicts_with = "quiet")]
    verbose: u8,
}

impl Args {
    pub(crate) fn log_verbosity(&self) -> LogVerbosity {
        match self.verbose {
            0 => LogVerbosity::Default,
            1 => LogVerbosity::Debug,
            2.. => LogVerbosity::Trace,
        }
    }
}

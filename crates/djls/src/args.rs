use clap::Parser;

#[derive(Parser, Debug, Clone)]
pub(crate) struct Args {
    /// Use verbose output.
    #[arg(global = true, action = clap::ArgAction::Count, long, short)]
    pub verbose: u8,
}

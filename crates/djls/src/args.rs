use clap::Parser;

#[derive(Parser)]
pub struct Args {
    #[command(flatten)]
    pub global: GlobalArgs,
}

#[derive(Parser, Debug, Clone)]
pub struct GlobalArgs {
    /// Do not print any output.
    #[arg(global = true, long, short, conflicts_with = "verbose")]
    pub quiet: bool,

    /// Use verbose output.
    #[arg(global = true, action = clap::ArgAction::Count, long, short, conflicts_with = "quiet")]
    pub verbose: u8,
}
//! CLI entry point for corpus management.

use camino::Utf8Path;
use camino::Utf8PathBuf;
use clap::Parser;
use clap::Subcommand;
use djls_corpus::manifest::Manifest;

#[derive(Parser)]
#[command(name = "djls-corpus", about = "Manage the Django template corpus")]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Path to manifest file
    #[arg(long)]
    manifest: Option<Utf8PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    /// Download and extract corpus packages/repos
    Sync,
    /// Remove all synced corpus data
    Clean,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let manifest_dir = Utf8Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest_path = cli
        .manifest
        .unwrap_or_else(|| manifest_dir.join("manifest.toml"));
    let manifest = Manifest::load(&manifest_path)?;
    let corpus_root = manifest.corpus_root(manifest_dir)?;

    match cli.command {
        Command::Sync => {
            eprintln!("Syncing corpus to {corpus_root}...");
            djls_corpus::sync::sync_corpus(&manifest, &corpus_root)?;
            eprintln!("Corpus synced to {corpus_root}");
        }
        Command::Clean => {
            if corpus_root.as_std_path().exists() {
                std::fs::remove_dir_all(corpus_root.as_std_path())?;
                eprintln!("Corpus cleaned");
            } else {
                eprintln!("No corpus to clean");
            }
        }
    }

    Ok(())
}

//! CLI entry point for corpus management.

use camino::Utf8Component;
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

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Command::Sync => corpus_sync(&cli),
        Command::Clean => corpus_clean(&cli),
    }
}

fn load_config(cli: &Cli) -> (Manifest, Utf8PathBuf) {
    let manifest_dir = Utf8Path::new(env!("CARGO_MANIFEST_DIR"));

    let manifest_path = cli
        .manifest
        .clone()
        .unwrap_or_else(|| manifest_dir.join("manifest.toml"));
    let manifest = Manifest::load(&manifest_path).expect("Failed to load manifest");

    let corpus_root = validated_corpus_root(&manifest);

    (manifest, corpus_root)
}

/// Build and validate the corpus root path from the manifest.
///
/// Rejects absolute paths and paths containing `..` components to prevent
/// `remove_dir_all` or file writes outside the crate directory.
fn validated_corpus_root(manifest: &Manifest) -> Utf8PathBuf {
    let root_dir = &manifest.corpus.root_dir;
    let root_path = Utf8Path::new(root_dir);

    assert!(
        !root_path.as_std_path().is_absolute(),
        "corpus root_dir must be a relative path, got: {root_dir}"
    );

    assert!(
        !root_path
            .components()
            .any(|c| matches!(c, Utf8Component::ParentDir)),
        "corpus root_dir must not contain '..' components, got: {root_dir}"
    );

    Utf8Path::new(env!("CARGO_MANIFEST_DIR")).join(root_dir)
}

fn corpus_sync(cli: &Cli) {
    let (manifest, corpus_root) = load_config(cli);

    eprintln!("Syncing corpus to {corpus_root}...");
    djls_corpus::sync::sync_corpus(&manifest, &corpus_root).expect("Failed to sync corpus");
    eprintln!("Corpus synced to {corpus_root}");
}

fn corpus_clean(cli: &Cli) {
    let (_manifest, corpus_root) = load_config(cli);

    if corpus_root.as_std_path().exists() {
        std::fs::remove_dir_all(corpus_root.as_std_path()).expect("Failed to remove corpus");
        eprintln!("Corpus cleaned");
    } else {
        eprintln!("No corpus to clean");
    }
}

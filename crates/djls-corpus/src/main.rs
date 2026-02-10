//! CLI entry point for corpus management.

use camino::Utf8Path;
use camino::Utf8PathBuf;
use clap::Parser;
use clap::Subcommand;
use djls_corpus::add::Bounds;
use djls_corpus::lock::LockFilter;
use djls_corpus::lock::Lockfile;
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
    /// Add `PyPI` packages to the manifest and update the lockfile
    Add {
        /// `PyPI` package names
        #[arg(required = true)]
        names: Vec<String>,

        /// Version pinning level
        #[arg(long, default_value = "exact")]
        bounds: Bounds,
    },
    /// Resolve latest versions and update the lockfile
    Lock {
        /// Package or repo names to lock (locks all if omitted)
        names: Vec<String>,
    },
    /// Download and extract corpus packages/repos from the lockfile
    Sync {
        /// Re-resolve versions before syncing, ignoring pinned versions in the lockfile
        #[arg(short = 'U', long)]
        upgrade: bool,

        /// Don't remove old versions after syncing
        #[arg(long)]
        no_prune: bool,
    },
    /// Remove synced corpus data (all by default, or specific packages/repos)
    Clean {
        /// Package or repo names to remove (removes all if omitted)
        names: Vec<String>,
    },
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    let manifest_dir = Utf8Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest_path = cli
        .manifest
        .unwrap_or_else(|| manifest_dir.join("manifest.toml"));
    let lockfile_path = manifest_path.with_extension("lock");

    match cli.command {
        Command::Add { names, bounds } => {
            djls_corpus::add::add_packages(&manifest_path, &names, bounds)?;
            update_lockfile(&manifest_path, &lockfile_path, &LockFilter::All)?;
        }
        Command::Lock { names } => {
            let filter = if names.is_empty() {
                LockFilter::All
            } else {
                LockFilter::Names(names)
            };
            update_lockfile(&manifest_path, &lockfile_path, &filter)?;
        }
        Command::Sync { upgrade, no_prune } => {
            if upgrade {
                update_lockfile(&manifest_path, &lockfile_path, &LockFilter::All)?;
            }

            let lockfile = Lockfile::load(&lockfile_path).map_err(|_| {
                anyhow::anyhow!(
                    "No lockfile found at {lockfile_path}. Run `djls-corpus lock` first."
                )
            })?;
            let manifest = Manifest::load(&manifest_path)?;
            let corpus_root = manifest.corpus_root(manifest_dir)?;

            tracing::info!(%corpus_root, "syncing corpus");
            djls_corpus::sync::sync_corpus(&lockfile, &corpus_root, !no_prune)?;
            tracing::info!(%corpus_root, "corpus synced");
        }
        Command::Clean { names } => {
            let manifest = Manifest::load(&manifest_path)?;
            let corpus_root = manifest.corpus_root(manifest_dir)?;

            if !corpus_root.as_std_path().exists() {
                tracing::info!("no corpus to clean");
                return Ok(());
            }

            if names.is_empty() {
                std::fs::remove_dir_all(corpus_root.as_std_path())?;
                tracing::info!("corpus cleaned");
            } else {
                djls_corpus::sync::clean_packages(&corpus_root, &names)?;
            }
        }
    }

    Ok(())
}

fn update_lockfile(
    manifest_path: &Utf8Path,
    lockfile_path: &Utf8Path,
    filter: &LockFilter,
) -> anyhow::Result<()> {
    let manifest = Manifest::load(manifest_path)?;
    let existing = if lockfile_path.as_std_path().exists() {
        Lockfile::load(lockfile_path)?
    } else {
        Lockfile::default()
    };

    tracing::info!("resolving latest versions");
    let lockfile = djls_corpus::lock::lock_corpus(&manifest, &existing, filter)?;
    lockfile.save(lockfile_path)?;
    tracing::info!(%lockfile_path, "lockfile updated");
    Ok(())
}

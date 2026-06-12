//! CLI entry point for corpus management.

use camino::Utf8Path;
use camino::Utf8PathBuf;
use clap::Parser;
use clap::Subcommand;
use djls_testing::LockFilter;
use djls_testing::Lockfile;
use djls_testing::Manifest;
use djls_testing::VendorSpecFixturesOptions;

#[derive(Parser)]
#[command(name = "corpus", about = "Manage the Django template corpus")]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Path to manifest file
    #[arg(long)]
    manifest: Option<Utf8PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    /// Resolve latest versions and update the lockfile
    Lock {
        /// Repo names to lock (locks all if omitted)
        names: Vec<String>,
    },
    /// Download and extract corpus repos from the lockfile
    Sync {
        /// Re-resolve versions before syncing, ignoring pinned versions in the lockfile
        #[arg(short = 'U', long)]
        upgrade: bool,

        /// Don't remove old versions after syncing
        #[arg(long)]
        no_prune: bool,
    },
    /// Remove synced corpus data (all by default, or specific repos)
    Clean {
        /// Repo names to remove (removes all if omitted)
        names: Vec<String>,
    },
    /// Regenerate vendored djls-project spec extraction fixtures from the synced corpus
    VendorSpecFixtures {
        /// Check whether generated fixtures match the working tree without writing changes
        #[arg(long)]
        check: bool,

        /// Fixture output directory (defaults to crates/djls-project/src/specs/testdata)
        #[arg(long)]
        output_dir: Option<Utf8PathBuf>,
    },
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    let default_manifest_dir = Utf8Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest_path = cli
        .manifest
        .unwrap_or_else(|| default_manifest_dir.join("manifest.toml"));
    let manifest_dir = manifest_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Invalid manifest path: no parent directory"))?;
    let lockfile_path = manifest_path.with_extension("lock");

    match cli.command {
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
                    "No lockfile found at {lockfile_path}. Run `cargo run -p djls-testing --bin corpus -- lock` first."
                )
            })?;
            let manifest = Manifest::load(&manifest_path)?;
            let corpus_root = manifest.corpus_root(manifest_dir);

            tracing::info!(%corpus_root, "syncing corpus");
            djls_testing::sync_corpus(&lockfile, &corpus_root, !no_prune)?;
            tracing::info!(%corpus_root, "corpus synced");
        }
        Command::Clean { names } => {
            let manifest = Manifest::load(&manifest_path)?;
            let corpus_root = manifest.corpus_root(manifest_dir);

            if !corpus_root.as_std_path().exists() {
                tracing::info!("no corpus to clean");
                return Ok(());
            }

            if names.is_empty() {
                std::fs::remove_dir_all(corpus_root.as_std_path())?;
                tracing::info!("corpus cleaned");
            } else {
                djls_testing::clean_entries(&corpus_root, &names)?;
            }
        }
        Command::VendorSpecFixtures { check, output_dir } => {
            djls_testing::vendor_spec_fixtures(VendorSpecFixturesOptions { check, output_dir })?;
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

    let licenses_dir = manifest_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Invalid manifest path: no parent directory"))?
        .join("licenses");

    tracing::info!("resolving latest versions");
    let (lockfile, errors) =
        djls_testing::lock_corpus(&manifest, &existing, filter, &licenses_dir)?;
    lockfile.save(lockfile_path)?;
    tracing::info!(%lockfile_path, "lockfile updated");

    if !errors.is_empty() {
        anyhow::bail!(
            "Failed to lock {} entries:\n  {}",
            errors.len(),
            errors.join("\n  ")
        );
    }

    Ok(())
}

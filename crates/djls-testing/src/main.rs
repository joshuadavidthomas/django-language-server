//! CLI entry point for corpus management.

use anyhow::Context as _;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use clap::Parser;
use clap::Subcommand;
use clap::ValueEnum;
use djls_project::Db as _;
use djls_project::file_to_module;
use djls_testing::Corpus;
use djls_testing::LockFilter;
use djls_testing::Lockfile;
use djls_testing::Manifest;
use djls_testing::VendorSpecFixturesOptions;
use djls_testing::extract_bundle;
use djls_testing::sorted_snapshot;

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
    /// Set up repository environments and extract facts in their project context
    Environment {
        #[arg(value_enum)]
        action: EnvironmentAction,
        /// Repository names (all locked repositories if omitted)
        names: Vec<String>,
    },
    /// Resolve latest versions and update the lockfile
    Lock {
        /// Repo names to lock (locks all if omitted)
        names: Vec<String>,
    },
    /// Prepare corpus source and Python environments from the lockfile
    Sync {
        /// Re-resolve versions before syncing, ignoring pinned versions in the lockfile
        #[arg(short = 'U', long)]
        upgrade: bool,

        /// Don't remove old versions after syncing
        #[arg(long)]
        no_prune: bool,

        /// Only download source fixtures; do not prepare runnable corpus environments
        #[arg(long)]
        source_only: bool,
    },
    /// Remove synced corpus data (all by default, or specific repos)
    Clean {
        /// Repo names to remove (removes all if omitted)
        names: Vec<String>,
    },
    /// Regenerate vendored spec snippets and bounded full-source fixtures from the synced corpus
    VendorSpecFixtures {
        /// Check whether generated fixtures match the working tree without writing changes
        #[arg(long)]
        check: bool,

        /// Snippet output directory; full sources go in its source/ child
        #[arg(long)]
        output_dir: Option<Utf8PathBuf>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum EnvironmentAction {
    Sync,
    Check,
    /// Emit one JSON object per extraction target, without updating snapshots
    Extract,
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
        Command::Environment { action, names } => {
            let corpus = Corpus::require_from_manifest(&manifest_path.canonicalize_utf8()?)?;
            run_environments(&corpus, action, names)?;
        }
        Command::Lock { names } => {
            let filter = if names.is_empty() {
                LockFilter::All
            } else {
                LockFilter::Names(names)
            };
            update_lockfile(&manifest_path, &lockfile_path, &filter)?;
        }
        Command::Sync {
            upgrade,
            no_prune,
            source_only,
        } => {
            if upgrade {
                update_lockfile(&manifest_path, &lockfile_path, &LockFilter::All)?;
            }

            let lockfile = Lockfile::load(&lockfile_path).map_err(|error| {
                anyhow::anyhow!(
                    "No valid lockfile found at {lockfile_path}. Run `cargo run -p djls-testing --bin corpus -- lock` first: {error}"
                )
            })?;
            let manifest = Manifest::load(&manifest_path)?;
            let corpus_root = manifest.corpus_root(manifest_dir);

            tracing::info!(%corpus_root, "syncing corpus");
            djls_testing::sync_corpus(&lockfile, &corpus_root, !no_prune)?;
            if !source_only {
                let corpus = Corpus::require_from_manifest(&manifest_path.canonicalize_utf8()?)?;
                run_environments(&corpus, EnvironmentAction::Sync, Vec::new())?;
                run_environments(&corpus, EnvironmentAction::Check, Vec::new())?;
            }
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

fn run_environments(
    corpus: &Corpus,
    action: EnvironmentAction,
    names: Vec<String>,
) -> anyhow::Result<()> {
    let all = names.is_empty();
    let names = if all {
        corpus
            .locked_repos()
            .map(|(name, _)| name.to_string())
            .collect()
    } else {
        names
    };
    let mut errors = Vec::new();
    for name in names {
        if all && let Some(reason) = corpus.environment_deferral(&name)? {
            tracing::warn!(%name, %reason, "environment deferred");
            continue;
        }
        let result = match action {
            EnvironmentAction::Sync => corpus.sync_environment(&name),
            EnvironmentAction::Check => corpus.environment_database(&name).map(|_| ()),
            EnvironmentAction::Extract => extract_environment(corpus, &name),
        };
        match result {
            Ok(()) => tracing::info!(%name, "environment operation succeeded"),
            Err(error) => errors.push(format!("{name}: {error:#}")),
        }
    }
    anyhow::ensure!(
        errors.is_empty(),
        "corpus environments failed:\n{}",
        errors.join("\n")
    );
    Ok(())
}

fn extract_environment(corpus: &Corpus, name: &str) -> anyhow::Result<()> {
    let db = corpus.environment_database(name)?;
    let project = db
        .project()
        .context("environment database has no project")?;
    for target in corpus
        .extraction_target_members()?
        .into_iter()
        .filter(|target| target.member == name)
    {
        let module = file_to_module(&db, project, target.path.clone()).with_context(|| {
            format!(
                "corpus `{name}` target `{}` does not resolve in its configured source roots",
                target.relative_path
            )
        })?;
        let bundle = extract_bundle(&db, module.file(), module.name().clone());
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "repository": name,
                "path": target.relative_path,
                "facts": sorted_snapshot(&bundle)?,
            }))?
        );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_only_sync_requires_an_explicit_opt_out() {
        let cli = Cli::try_parse_from(["corpus", "sync"]).expect("default sync");
        assert!(matches!(
            cli.command,
            Command::Sync {
                source_only: false,
                upgrade: false,
                no_prune: false
            }
        ));
        let cli = Cli::try_parse_from(["corpus", "sync", "--source-only", "-U", "--no-prune"])
            .expect("source-only sync with existing flags");
        assert!(matches!(
            cli.command,
            Command::Sync {
                source_only: true,
                upgrade: true,
                no_prune: true
            }
        ));
    }
}

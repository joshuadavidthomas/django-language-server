//! CLI entry point for corpus management.

use camino::Utf8Path;
use camino::Utf8PathBuf;
use clap::Parser;
use clap::Subcommand;
use clap::ValueEnum;
use djls_corpus::bump::BumpFilter;
use djls_corpus::lockfile::Lockfile;
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

#[derive(Clone, Copy, ValueEnum)]
enum Bounds {
    Major,
    Minor,
    Exact,
}

#[derive(Subcommand)]
enum Command {
    /// Add `PyPI` packages to the manifest and update the lockfile
    Add {
        /// `PyPI` package names
        names: Vec<String>,

        /// Version pinning level [default: exact]
        #[arg(long, default_value = "exact")]
        bounds: Bounds,
    },
    /// Resolve latest versions and update the lockfile (all by default)
    Bump {
        /// Package or repo names to bump (bumps all if omitted)
        names: Vec<String>,
    },
    /// Download and extract corpus packages/repos from the lockfile
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
    let lockfile_path = manifest_path.with_extension("lock");

    match cli.command {
        Command::Add { names, bounds } => {
            if names.is_empty() {
                anyhow::bail!("Specify one or more package names");
            }
            for name in &names {
                add_package(&manifest_path, name, bounds)?;
            }
            bump_lockfile(&manifest_path, &lockfile_path, &BumpFilter::All)?;
        }
        Command::Bump { names } => {
            let filter = if names.is_empty() {
                BumpFilter::All
            } else {
                BumpFilter::Names(names)
            };
            bump_lockfile(&manifest_path, &lockfile_path, &filter)?;
        }
        Command::Sync => {
            let lockfile = Lockfile::load(&lockfile_path).map_err(|_| {
                anyhow::anyhow!(
                    "No lockfile found at {lockfile_path}. Run `djls-corpus bump` first."
                )
            })?;
            let manifest = Manifest::load(&manifest_path)?;
            let corpus_root = manifest.corpus_root(manifest_dir)?;

            eprintln!("Syncing corpus to {corpus_root}...");
            djls_corpus::sync::sync_corpus(&lockfile, &corpus_root)?;
            eprintln!("Corpus synced to {corpus_root}");
        }
        Command::Clean => {
            let manifest = Manifest::load(&manifest_path)?;
            let corpus_root = manifest.corpus_root(manifest_dir)?;

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

fn bump_lockfile(
    manifest_path: &Utf8Path,
    lockfile_path: &Utf8Path,
    filter: &BumpFilter,
) -> anyhow::Result<()> {
    let manifest = Manifest::load(manifest_path)?;
    let existing = if lockfile_path.as_std_path().exists() {
        Lockfile::load(lockfile_path)?
    } else {
        Lockfile::default()
    };

    eprintln!("Resolving latest versions...");
    let lockfile = djls_corpus::bump::bump_corpus(&manifest, &existing, filter)?;
    lockfile.save(lockfile_path)?;
    eprintln!("Updated {lockfile_path}");
    Ok(())
}

fn add_package(manifest_path: &Utf8Path, name: &str, bounds: Bounds) -> anyhow::Result<()> {
    let (_, latest) = djls_corpus::bump::resolve_pypi_latest(name)?;

    let parts: Vec<&str> = latest.split('.').collect();
    let version_spec = match bounds {
        Bounds::Major => parts[..1].join("."),
        Bounds::Minor if parts.len() >= 2 => parts[..2].join("."),
        Bounds::Minor | Bounds::Exact => latest.clone(),
    };

    let content = std::fs::read_to_string(manifest_path.as_std_path())?;
    let mut doc = content
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| anyhow::anyhow!("Failed to parse manifest: {e}"))?;

    let packages = doc["package"]
        .as_array_of_tables_mut()
        .ok_or_else(|| anyhow::anyhow!("No [[package]] array in manifest"))?;

    // Remove existing entry if present
    let mut i = 0;
    while i < packages.len() {
        let is_match = packages
            .get(i)
            .and_then(|t| t.get("name"))
            .and_then(|v| v.as_str())
            .is_some_and(|n| n == name);
        if is_match {
            packages.remove(i);
        } else {
            i += 1;
        }
    }

    // Find sorted insertion point
    let mut insert_at = packages.len();
    for (i, table) in packages.iter().enumerate() {
        let Some(existing_name) = table.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        if existing_name > name {
            insert_at = i;
            break;
        }
    }

    let mut entry = toml_edit::Table::new();
    entry.insert("name", toml_edit::value(name));
    entry.insert("version", toml_edit::value(&version_spec));

    // toml_edit only has push(); rebuild with insertion at the right position
    let mut tables: Vec<toml_edit::Table> = Vec::new();
    for (i, table) in packages.iter().enumerate() {
        if i == insert_at {
            tables.push(entry.clone());
        }
        tables.push(table.clone());
    }
    if insert_at >= packages.len() {
        tables.push(entry);
    }

    while !packages.is_empty() {
        packages.remove(0);
    }
    for t in tables {
        packages.push(t);
    }

    let output = doc.to_string();
    let trimmed = output.trim_end().to_string() + "\n";
    std::fs::write(manifest_path.as_std_path(), trimmed)?;

    eprintln!("Added {name} {version_spec} (latest: {latest})");
    Ok(())
}

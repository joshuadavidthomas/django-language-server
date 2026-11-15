//! Compare DJLS's Django facts with real Django, on corpus repositories or any project.
//!
//! ```bash
//! # Corpus repositories (after `just corpus sync`):
//! cargo run -p djls-testing --bin django-oracle -- corpus misago sorl-thumbnail
//!
//! # Your own project or package:
//! cargo run -p djls-testing --bin django-oracle -- project --root . --venv .venv \
//!     --settings mysite.settings
//! ```

use anyhow::Context as _;
use camino::Utf8PathBuf;
use clap::Parser;
use clap::Subcommand;
use clap::ValueEnum;
use djls_testing::Corpus;
use djls_testing::oracle::OracleOptions;
use djls_testing::oracle::OracleReport;
use djls_testing::oracle::OracleTarget;
use djls_testing::oracle::corpus_target;
use djls_testing::oracle::discover_library_files;
use djls_testing::oracle::render_markdown;
use djls_testing::oracle::run_oracle;

#[derive(Parser)]
#[command(
    name = "django-oracle",
    about = "Report where DJLS's Django facts disagree with Django"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Write the JSON report here.
    #[arg(long, global = true)]
    json: Option<Utf8PathBuf>,

    /// Write the Markdown report here (default: stdout).
    #[arg(long, global = true)]
    markdown: Option<Utf8PathBuf>,

    /// Generate observations without accepting them. The exact ledger checker owns CI policy.
    #[arg(long, global = true)]
    report_only: bool,

    /// Existing template files to replay as permanent seeds.
    #[arg(long, global = true)]
    seeds: Option<Utf8PathBuf>,

    /// Django backend name. Required when project settings configure several backends.
    #[arg(long, global = true)]
    backend: Option<String>,

    /// Also probe Django's own libraries (`django.*`) tag by tag.
    #[arg(long, global = true)]
    include_django: bool,

    /// Which settings to compare under. `both` runs project settings for loading and template
    /// resolution, then generated settings that isolate each library's own facts.
    #[arg(long, global = true, value_enum, default_value_t = Modes::Both)]
    settings_mode: Modes,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Modes {
    Project,
    Synthetic,
    Both,
}

#[derive(Subcommand)]
enum Command {
    /// Check explicitly named corpus repositories with their prepared environments.
    Corpus { names: Vec<String> },
    /// Check a project or package with its own virtual environment.
    Project {
        /// Project root, as DJLS is configured.
        #[arg(long, default_value = ".")]
        root: Utf8PathBuf,
        /// Virtual environment that has Django and the project's dependencies.
        #[arg(long)]
        venv: Utf8PathBuf,
        /// Django settings module. Omit to check every `templatetags` library under the root
        /// with generated settings.
        #[arg(long)]
        settings: Option<String>,
        /// Extra Python path entries, as the `pythonpath` setting.
        #[arg(long)]
        pythonpath: Vec<Utf8PathBuf>,
        /// Apps to install in generated settings (repeatable), when there is no settings module.
        #[arg(long = "app")]
        apps: Vec<String>,
        /// Environment variables as `KEY=VALUE` (repeatable), given to both Django and DJLS.
        #[arg(long = "env", value_parser = parse_env)]
        env: Vec<(String, String)>,
    },
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,django_oracle=info")),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();

    let targets = targets(cli.command)?;

    let mut reports: Vec<OracleReport> = Vec::new();
    let mut failures = Vec::new();
    for target in &targets {
        anyhow::ensure!(
            cli.settings_mode != Modes::Project || target.settings_module.is_some(),
            "project mode requires a settings module for {}",
            target.name
        );
        let mut runs = Vec::new();
        if cli.settings_mode != Modes::Synthetic && target.settings_module.is_some() {
            runs.push(false);
        }
        if cli.settings_mode != Modes::Project || target.settings_module.is_none() {
            runs.push(true);
        }
        for synthetic in runs {
            tracing::info!(name = %target.name, synthetic, "running Django oracle");
            let options = OracleOptions {
                include_django: cli.include_django,
                synthetic,
                backend: cli.backend.clone(),
                seeds: cli.seeds.clone(),
                ..OracleOptions::default()
            };
            match run_oracle(target, &options) {
                Ok(report) => reports.push(report),
                Err(error) => failures.push(format!("{}: {error:#}", target.name)),
            }
        }
    }

    if let Some(path) = &cli.json {
        std::fs::write(path, serde_json::to_string_pretty(&reports)?)?;
    }
    let markdown = render_markdown(&reports)?;
    match &cli.markdown {
        Some(path) => std::fs::write(path, markdown)?,
        None => print!("{markdown}"),
    }
    for failure in &failures {
        tracing::error!("{failure}");
    }

    if !failures.is_empty()
        || (!cli.report_only && reports.iter().any(|report| !report.findings.is_empty()))
    {
        std::process::exit(1);
    }
    Ok(())
}

fn targets(command: Command) -> anyhow::Result<Vec<OracleTarget>> {
    Ok(match command {
        Command::Corpus { names } => {
            let corpus = Corpus::require()?;
            anyhow::ensure!(
                !names.is_empty(),
                "name the corpus targets explicitly; an empty run cannot pass"
            );
            names
                .iter()
                .map(|name| corpus_target(&corpus, name))
                .collect::<anyhow::Result<Vec<_>>>()?
        }
        Command::Project {
            root,
            venv,
            settings,
            pythonpath,
            apps,
            env,
        } => {
            let root = root.canonicalize_utf8().context("project root")?;
            let venv = venv.canonicalize_utf8().context("virtual environment")?;
            let pythonpath = pythonpath
                .iter()
                .map(|path| path.canonicalize_utf8())
                .collect::<Result<Vec<_>, _>>()?;
            vec![OracleTarget {
                name: root.file_name().unwrap_or("project").to_string(),
                library_files: discover_library_files(&root),
                disk_roots: std::iter::once(root.clone())
                    .chain(pythonpath.iter().cloned())
                    .collect(),
                python: venv.join("bin/python"),
                environment_root: venv,
                project_root: root,
                source_roots: pythonpath,
                settings_module: settings,
                apps,
                env,
            }]
        }
    })
}

fn parse_env(value: &str) -> Result<(String, String), String> {
    value
        .split_once('=')
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .ok_or_else(|| format!("expected KEY=VALUE, got `{value}`"))
}

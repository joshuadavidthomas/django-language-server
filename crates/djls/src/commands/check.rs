use std::io::Read as _;
use std::sync::Arc;

use anyhow::bail;
use anyhow::Context;
use anyhow::Result;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use clap::Parser;
use djls_db::DjangoDatabase;
use djls_db::FileCheckResult;
use djls_source::Db as SourceDb;
use djls_source::DiagnosticRenderer;
use djls_workspace::OsFileSystem;
use djls_workspace::WalkOptions;

use crate::args::Args;
use crate::commands::common::discover_files;
use crate::commands::common::resolve_project_root;
use crate::commands::common::ColorMode;
use crate::commands::Command;
use crate::exit::Exit;

#[derive(Debug, Parser)]
pub struct Check {
    /// Files or directories to check. Pass `-` to read from stdin. If
    /// omitted, discovers template directories from the Django project.
    paths: Vec<Utf8PathBuf>,

    /// Select specific diagnostic codes to enable (e.g. S100,S101).
    #[arg(long, value_delimiter = ',')]
    select: Vec<String>,

    /// Ignore specific diagnostic codes (e.g. S108,S109).
    #[arg(long, value_delimiter = ',')]
    ignore: Vec<String>,

    /// Include hidden files and directories (those starting with `.`).
    #[arg(short = '.', long, default_value_t = false)]
    hidden: bool,

    /// Include or exclude files matching a glob pattern. Prefix with `!` to
    /// exclude. May be specified multiple times. Later patterns take
    /// precedence.
    #[arg(short = 'g', long = "glob")]
    globs: Vec<String>,

    /// Don't respect ignore files (.gitignore, .ignore, etc.).
    #[arg(long, default_value_t = false)]
    no_ignore: bool,

    /// Follow symbolic links.
    #[arg(short = 'L', long, default_value_t = false)]
    follow: bool,

    /// Limit directory traversal depth.
    #[arg(short = 'd', long)]
    max_depth: Option<usize>,

    /// When to use colors.
    #[arg(long, value_enum, default_value_t = ColorMode::Auto)]
    color: ColorMode,
}

impl Command for Check {
    fn execute(&self, args: &Args) -> Result<Exit> {
        let project_root = resolve_project_root()?;
        let settings =
            djls_conf::Settings::new(&project_root, None).context("Failed to load settings")?;

        let config = build_diagnostics_config(&settings, &self.select, &self.ignore);
        let fmt = pick_renderer(&self.color);
        let quiet = args.quiet;

        let reading_stdin = self.paths.iter().any(|path| path.as_str() == "-");
        let has_non_stdin_path = self.paths.iter().any(|path| path.as_str() != "-");

        if reading_stdin && has_non_stdin_path {
            bail!("Cannot mix `-` (stdin) with file or directory paths");
        }

        if reading_stdin {
            return check_stdin(&project_root, &settings, &config, &fmt, quiet);
        }

        let fs: Arc<dyn djls_workspace::FileSystem> = Arc::new(OsFileSystem);
        let db = DjangoDatabase::new(fs, &settings, Some(&project_root));

        let walk_options = WalkOptions {
            hidden: self.hidden,
            globs: self.globs.clone(),
            no_ignore: self.no_ignore,
            follow_links: self.follow,
            max_depth: self.max_depth,
        };

        let files = discover_files(&self.paths, &db, &project_root, &walk_options);

        if files.is_empty() {
            return Ok(Exit::success());
        }

        // DjangoDatabase is Send + !Sync (salsa::Storage has RefCell).
        // Clone the db per rayon task (each clone gets its own Salsa cache).
        // Collect raw diagnostic data in parallel, render on the main thread
        // after — the renderer is not Send and doesn't need to be.
        let mut raw_results: Vec<FileCheckResult> = {
            let db = db;
            let (tx, rx) = std::sync::mpsc::channel();

            rayon::scope(move |scope| {
                for path in files {
                    let db = db.clone();
                    let tx = tx.clone();
                    scope.spawn(move |_| {
                        let result = check_file_with_source(&db, &path);
                        if result.check.has_diagnostics() {
                            let _ = tx.send(result);
                        }
                    });
                }
            });

            rx.into_iter().collect()
        };
        raw_results.sort_by(|left, right| left.path.cmp(&right.path));

        let mut error_count: usize = 0;
        let mut file_count: usize = 0;

        for result in &raw_results {
            let rendered = result.render(&config, &fmt);
            if !rendered.is_empty() {
                file_count += 1;
                if !quiet {
                    for output in &rendered {
                        println!("{output}\n");
                    }
                }
                error_count += rendered.len();
            }
        }

        if error_count > 0 {
            if quiet {
                Ok(Exit::error())
            } else {
                let file_word = if file_count == 1 { "file" } else { "files" };
                let error_word = if error_count == 1 { "error" } else { "errors" };
                Ok(Exit::error().with_message(format!(
                    "Found {error_count} {error_word} in {file_count} {file_word}."
                )))
            }
        } else {
            Ok(Exit::success())
        }
    }
}

fn check_stdin(
    project_root: &Utf8Path,
    settings: &djls_conf::Settings,
    config: &djls_conf::DiagnosticsConfig,
    fmt: &DiagnosticRenderer,
    quiet: bool,
) -> Result<Exit> {
    let mut source = String::new();
    std::io::stdin()
        .read_to_string(&mut source)
        .context("Failed to read stdin")?;

    let mut mem_fs = djls_workspace::InMemoryFileSystem::new();
    let stdin_path = Utf8PathBuf::from("<stdin>.html");
    mem_fs.add_file(stdin_path.clone(), source);
    let fs: Arc<dyn djls_workspace::FileSystem> = Arc::new(mem_fs);
    let db = DjangoDatabase::new(fs, settings, Some(project_root));

    let result = check_file_with_source(&db, &stdin_path);
    let rendered = result.render(config, fmt);
    if rendered.is_empty() {
        Ok(Exit::success())
    } else if quiet {
        Ok(Exit::error())
    } else {
        for output in &rendered {
            println!("{output}\n");
        }
        let count = rendered.len();
        let word = if count == 1 { "error" } else { "errors" };
        Ok(Exit::error().with_message(format!("Found {count} {word}.")))
    }
}

/// Run `check_file` and capture the source text for later rendering.
fn check_file_with_source(db: &DjangoDatabase, path: &Utf8Path) -> FileCheckResult {
    let file = db.get_or_create_file(path);
    let source = file.source(db).to_string();
    let check = djls_db::check_file(db, file);

    FileCheckResult {
        path: path.to_owned(),
        source,
        check,
    }
}

fn build_diagnostics_config(
    settings: &djls_conf::Settings,
    select: &[String],
    ignore: &[String],
) -> djls_conf::DiagnosticsConfig {
    let mut config = settings.diagnostics().clone();

    for code in select {
        config.set_severity(code, djls_conf::DiagnosticSeverity::Error);
    }

    for code in ignore {
        config.set_severity(code, djls_conf::DiagnosticSeverity::Off);
    }

    config
}

fn pick_renderer(color: &ColorMode) -> DiagnosticRenderer {
    if color.should_use_color() {
        DiagnosticRenderer::styled()
    } else {
        DiagnosticRenderer::plain()
    }
}

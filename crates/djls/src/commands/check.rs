use std::io::IsTerminal;
use std::io::Read as _;
use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use clap::Parser;
use clap::ValueEnum;
use djls_db::DjangoDatabase;
use djls_ide::render_template_error;
use djls_ide::render_validation_error;
use djls_semantic::Db as SemanticDb;
use djls_semantic::ValidationError;
use djls_semantic::ValidationErrorAccumulator;
use djls_source::Db as SourceDb;
use djls_source::DiagnosticRenderer;
use djls_source::FileKind;
use djls_source::Span;
use djls_templates::TemplateError;
use djls_templates::TemplateErrorAccumulator;
use djls_workspace::walk_files;
use djls_workspace::OsFileSystem;
use djls_workspace::WalkOptions;

use crate::args::Args;
use crate::commands::Command;
use crate::exit::Exit;

#[derive(Clone, Debug, Default, ValueEnum)]
enum ColorMode {
    /// Use colors when output is a terminal.
    #[default]
    Auto,
    /// Always use colors.
    Always,
    /// Never use colors.
    Never,
}

#[derive(Debug, Parser)]
#[allow(clippy::struct_excessive_bools)]
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

        let reading_stdin = self.paths.iter().any(|p| p.as_str() == "-");

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
        let raw_results: Vec<FileCheckResult> = {
            let db = db;
            let (tx, rx) = std::sync::mpsc::channel();

            rayon::scope(move |scope| {
                for path in files {
                    let db = db.clone();
                    let tx = tx.clone();
                    scope.spawn(move |_| {
                        let result = check_file(&db, &path);
                        if result.has_diagnostics() {
                            let _ = tx.send(result);
                        }
                    });
                }
            });

            rx.into_iter().collect()
        };

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

fn discover_files(
    paths: &[Utf8PathBuf],
    db: &DjangoDatabase,
    project_root: &Utf8Path,
    options: &WalkOptions,
) -> Vec<Utf8PathBuf> {
    if !paths.is_empty() {
        let resolved: Vec<Utf8PathBuf> = paths
            .iter()
            .map(|p| {
                if p.is_relative() {
                    project_root.join(p)
                } else {
                    p.clone()
                }
            })
            .collect();
        return walk_files(&resolved, is_template, options);
    }

    if let Some(dirs) = db.template_dirs() {
        let dirs: Vec<Utf8PathBuf> = dirs.into_iter().collect();
        walk_files(&dirs, is_template, options)
    } else {
        walk_files(&[project_root.to_owned()], is_template, options)
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

    let result = check_file(&db, &stdin_path);
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

/// Raw diagnostic data collected for a single file.
///
/// Produced in parallel by rayon tasks (only Salsa queries, no rendering).
/// Rendered on the main thread after the parallel phase completes.
struct FileCheckResult {
    path: Utf8PathBuf,
    source: String,
    template_errors: Vec<TemplateError>,
    validation_errors: Vec<ValidationError>,
}

impl FileCheckResult {
    fn has_diagnostics(&self) -> bool {
        !self.template_errors.is_empty() || !self.validation_errors.is_empty()
    }

    fn render(
        &self,
        config: &djls_conf::DiagnosticsConfig,
        fmt: &DiagnosticRenderer,
    ) -> Vec<String> {
        let mut results = Vec::new();
        let path = self.path.as_str();
        let source = self.source.as_str();

        for error in &self.template_errors {
            if let Some(output) = render_template_error(source, path, error, config, fmt) {
                results.push(output);
            }
        }

        for error in &self.validation_errors {
            if let Some(output) = render_validation_error(source, path, error, config, fmt) {
                results.push(output);
            }
        }

        results
    }
}

fn check_file(db: &DjangoDatabase, path: &Utf8Path) -> FileCheckResult {
    let file = db.get_or_create_file(path);
    let source = file.source(db).to_string();

    let nodelist = djls_templates::parse_template(db, file);

    let template_errors: Vec<TemplateError> =
        djls_templates::parse_template::accumulated::<TemplateErrorAccumulator>(db, file)
            .iter()
            .map(|acc| acc.0.clone())
            .collect();

    let mut validation_errors: Vec<ValidationError> = Vec::new();

    if let Some(nodelist) = nodelist {
        djls_semantic::validate_nodelist(db, nodelist);

        let accumulated = djls_semantic::validate_nodelist::accumulated::<ValidationErrorAccumulator>(
            db, nodelist,
        );

        validation_errors = accumulated.iter().map(|acc| acc.0.clone()).collect();
        validation_errors.sort_by_key(|e| e.primary_span().map_or(0, Span::start));
    }

    FileCheckResult {
        path: path.to_owned(),
        source,
        template_errors,
        validation_errors,
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

fn resolve_project_root() -> Result<Utf8PathBuf> {
    let cwd = std::env::current_dir().context("Failed to get current directory")?;
    Utf8PathBuf::from_path_buf(cwd)
        .map_err(|_| anyhow::anyhow!("Current directory is not valid UTF-8"))
}

fn is_template(path: &Utf8Path) -> bool {
    FileKind::is_template(path)
}

fn pick_renderer(color: &ColorMode) -> DiagnosticRenderer {
    let use_color = match color {
        ColorMode::Always => true,
        ColorMode::Never => false,
        ColorMode::Auto => std::io::stdout().is_terminal(),
    };
    if use_color {
        DiagnosticRenderer::styled()
    } else {
        DiagnosticRenderer::plain()
    }
}

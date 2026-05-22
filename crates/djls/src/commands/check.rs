use std::io::Read as _;
use std::io::Write as _;
use std::sync::Arc;

use anyhow::bail;
use anyhow::Context;
use anyhow::Result;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use clap::Parser;
use djls_db::DjangoDatabase;
use djls_semantic::ValidationError;
use djls_semantic::ValidationErrorAccumulator;
use djls_source::Db as _;
use djls_source::Diagnostic;
use djls_source::DiagnosticRenderer;
use djls_source::File;
use djls_source::Severity;
use djls_source::SourceText;
use djls_source::Span;
use djls_templates::TemplateError;
use djls_templates::TemplateErrorAccumulator;
use djls_workspace::OsFileSystem;
use djls_workspace::WalkOptions;

use crate::args::Args;
use crate::commands::common::discover_files;
use crate::commands::common::resolve_project_root;
use crate::commands::common::ColorMode;
use crate::commands::Command;
use crate::exit::Exit;
use crate::loading::CliLoadingExecutor;

struct CheckResult {
    template_errors: Vec<TemplateError>,
    validation_errors: Vec<ValidationError>,
}

impl CheckResult {
    fn has_diagnostics(&self) -> bool {
        !self.template_errors.is_empty() || !self.validation_errors.is_empty()
    }
}

struct FileCheckResult {
    path: Utf8PathBuf,
    source: SourceText,
    check: CheckResult,
}

impl FileCheckResult {
    fn renderable_diagnostic_count(&self, config: &djls_conf::DiagnosticsConfig) -> usize {
        self.check
            .template_errors
            .iter()
            .filter(|error| diagnostic_is_enabled(config, error.diagnostic_code()))
            .count()
            + self
                .check
                .validation_errors
                .iter()
                .filter(|error| {
                    diagnostic_is_enabled(config, error.code()) && error.primary_span().is_some()
                })
                .count()
    }

    fn render(
        &self,
        config: &djls_conf::DiagnosticsConfig,
        fmt: &DiagnosticRenderer,
    ) -> Vec<String> {
        let mut results = Vec::with_capacity(self.renderable_diagnostic_count(config));
        let path = self.path.as_str();
        let source = self.source.as_str();

        for error in &self.check.template_errors {
            if let Some(output) = render_template_error(source, path, error, config, fmt) {
                results.push(output);
            }
        }

        for error in &self.check.validation_errors {
            if let Some(output) = render_validation_error(source, path, error, config, fmt) {
                results.push(output);
            }
        }

        results
    }
}

#[derive(Debug, Parser)]
pub(crate) struct Check {
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
        let fmt = pick_renderer(self.color);
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
        let mut db = DjangoDatabase::new(fs, &settings);
        db.bootstrap_project(&project_root, &settings);
        let mut loading = CliLoadingExecutor::new(&mut db, vec![project_root.clone()]);
        let mut observer = djls_project::NoopLoadingObserver;
        djls_project::run_loading_plan(
            djls_project::LoadingPlan::phase3(),
            &mut loading,
            &mut observer,
        );

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

        if quiet {
            for result in &raw_results {
                let count = result.renderable_diagnostic_count(&config);
                if count > 0 {
                    file_count += 1;
                    error_count += count;
                }
            }
        } else {
            let mut stdout = std::io::stdout().lock();
            for result in &raw_results {
                let rendered = result.render(&config, &fmt);
                if !rendered.is_empty() {
                    file_count += 1;
                    for output in &rendered {
                        writeln!(stdout, "{output}\n")?;
                    }
                    error_count += rendered.len();
                }
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
    let mut db = DjangoDatabase::new(fs, settings);
    db.bootstrap_project(project_root, settings);

    let result = check_file_with_source(&db, &stdin_path);
    if quiet {
        return if result.renderable_diagnostic_count(config) > 0 {
            Ok(Exit::error())
        } else {
            Ok(Exit::success())
        };
    }

    let rendered = result.render(config, fmt);
    if rendered.is_empty() {
        Ok(Exit::success())
    } else {
        let mut stdout = std::io::stdout().lock();
        for output in &rendered {
            writeln!(stdout, "{output}\n")?;
        }
        let count = rendered.len();
        let word = if count == 1 { "error" } else { "errors" };
        Ok(Exit::error().with_message(format!("Found {count} {word}.")))
    }
}

/// Run validation and capture the source text for later rendering.
fn check_file_with_source(db: &DjangoDatabase, path: &Utf8Path) -> FileCheckResult {
    let file = db.get_or_create_file(path);
    let source = file.source(db);
    let check = check_file(db, file);

    FileCheckResult {
        path: path.to_owned(),
        source,
        check,
    }
}

fn check_file(db: &dyn djls_semantic::Db, file: File) -> CheckResult {
    djls_semantic::validate_template_file(db, file);

    let template_errors: Vec<TemplateError> =
        djls_templates::parse_template::accumulated::<TemplateErrorAccumulator>(db, file)
            .iter()
            .map(|acc| acc.0.clone())
            .collect();

    let accumulated =
        djls_semantic::validate_template_file::accumulated::<ValidationErrorAccumulator>(db, file);

    let mut validation_errors: Vec<ValidationError> =
        accumulated.iter().map(|acc| acc.0.clone()).collect();
    validation_errors.sort_by_cached_key(|e| e.primary_span().map_or(0, Span::start));

    CheckResult {
        template_errors,
        validation_errors,
    }
}

fn diagnostic_is_enabled(config: &djls_conf::DiagnosticsConfig, code: &str) -> bool {
    config.get_severity(code) != djls_conf::DiagnosticSeverity::Off
}

fn to_render_severity(severity: djls_conf::DiagnosticSeverity) -> Severity {
    match severity {
        djls_conf::DiagnosticSeverity::Error => Severity::Error,
        djls_conf::DiagnosticSeverity::Warning => Severity::Warning,
        djls_conf::DiagnosticSeverity::Info => Severity::Info,
        djls_conf::DiagnosticSeverity::Hint | djls_conf::DiagnosticSeverity::Off => Severity::Hint,
    }
}

fn render_template_error(
    source: &str,
    path: &str,
    error: &TemplateError,
    config: &djls_conf::DiagnosticsConfig,
    fmt: &DiagnosticRenderer,
) -> Option<String> {
    let code = error.diagnostic_code();
    let severity = config.get_severity(code);
    if severity == djls_conf::DiagnosticSeverity::Off {
        return None;
    }

    let message = error.to_string();
    let span = error.primary_span().map_or_else(
        || Span::new(0, 0),
        |(start, length)| Span::new(start, length),
    );
    let diag = Diagnostic::new(
        source,
        path,
        code,
        &message,
        to_render_severity(severity),
        span,
        "",
    );
    Some(fmt.render(&diag))
}

fn render_validation_error(
    source: &str,
    path: &str,
    error: &ValidationError,
    config: &djls_conf::DiagnosticsConfig,
    fmt: &DiagnosticRenderer,
) -> Option<String> {
    let code = error.code();
    let severity = config.get_severity(code);
    if severity == djls_conf::DiagnosticSeverity::Off {
        return None;
    }

    let span = error.primary_span()?;
    let message = error.to_string();
    let render_severity = to_render_severity(severity);

    let mut diag = Diagnostic::new(source, path, code, &message, render_severity, span, "");

    if let ValidationError::UnbalancedStructure {
        closing_span: Some(cs),
        ..
    } = error
    {
        diag = diag.annotation(*cs, "", false);
    }

    Some(fmt.render(&diag))
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

fn pick_renderer(color: ColorMode) -> DiagnosticRenderer {
    if color.should_use_color() {
        DiagnosticRenderer::styled()
    } else {
        DiagnosticRenderer::plain()
    }
}

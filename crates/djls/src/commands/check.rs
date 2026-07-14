use std::io::Read as _;
use std::io::Write as _;
use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use clap::Parser;
use djls_db::DjangoDatabase;
use djls_semantic::ValidationError;
use djls_semantic::ValidationErrorAccumulator;
use djls_source::CaseSensitivity;
use djls_source::Diagnostic;
use djls_source::DiagnosticRenderer;
use djls_source::File;
use djls_source::FileSystem;
use djls_source::OsFileSystem;
use djls_source::RootWalk;
use djls_source::Severity;
use djls_source::SourceText;
use djls_source::Span;
use djls_source::WalkOptions;
use djls_source::path_to_file;
use djls_templates::TemplateError;
use djls_templates::TemplateErrorAccumulator;

use crate::args::Args;
use crate::commands::Command;
use crate::commands::common::ColorMode;
use crate::commands::common::discover_files;
use crate::commands::common::resolve_project_root;
use crate::exit::Exit;

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

enum CheckInput {
    Files {
        file_system: Arc<dyn FileSystem>,
    },
    Stdin {
        file_system: Arc<dyn FileSystem>,
        path: Utf8PathBuf,
    },
}

impl CheckInput {
    fn collect(paths: &[Utf8PathBuf]) -> Result<Self> {
        let mut reads_stdin = false;
        let mut has_file_paths = false;

        for path in paths {
            if path.as_str() == "-" {
                reads_stdin = true;
            } else {
                has_file_paths = true;
            }

            if reads_stdin && has_file_paths {
                bail!("Cannot mix `-` (stdin) with file or directory paths");
            }
        }

        if !reads_stdin {
            return Ok(Self::Files {
                file_system: Arc::new(OsFileSystem::default()),
            });
        }

        let mut source = String::new();
        std::io::stdin()
            .read_to_string(&mut source)
            .context("Failed to read stdin")?;

        let path = Utf8PathBuf::from("<stdin>.html");
        Ok(Self::Stdin {
            file_system: Arc::new(SingleFileOverlay::new(
                path.clone(),
                source,
                OsFileSystem::default(),
            )),
            path,
        })
    }

    fn file_system(&self) -> Arc<dyn FileSystem> {
        match self {
            Self::Files { file_system } | Self::Stdin { file_system, .. } => file_system.clone(),
        }
    }

    fn files(
        &self,
        requested_paths: &[Utf8PathBuf],
        db: &DjangoDatabase,
        project_root: &Utf8Path,
        walk_options: &WalkOptions,
    ) -> Vec<Utf8PathBuf> {
        match self {
            Self::Files { .. } => discover_files(requested_paths, db, project_root, walk_options),
            Self::Stdin { path, .. } => vec![path.clone()],
        }
    }

    const fn summary(&self) -> SummaryStyle {
        match self {
            Self::Files { .. } => SummaryStyle::Files,
            Self::Stdin { .. } => SummaryStyle::Stdin,
        }
    }
}

#[derive(Clone, Copy)]
enum SummaryStyle {
    Files,
    Stdin,
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
    /// Template files or directories to check. Pass `-` to read stdin; stdin is
    /// analyzed as a generic Template in the current Project; stdin cannot be
    /// combined with paths. If omitted, discovers Template directories from the
    /// Django project.
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
        let input = CheckInput::collect(&self.paths)?;

        let config = build_diagnostics_config(&settings, &self.select, &self.ignore);
        let fmt = pick_renderer(self.color);
        let quiet = args.quiet;

        let mut db = DjangoDatabase::new(input.file_system(), &settings, Some(&project_root));
        db.apply_project_settings(settings);
        djls_project::run_django_discovery(&mut db).context("No Project configured for check")?;

        let walk_options = WalkOptions {
            hidden: self.hidden,
            globs: self.globs.clone(),
            no_ignore: self.no_ignore,
            follow_links: self.follow,
            max_depth: self.max_depth,
        };
        let files = input.files(&self.paths, &db, &project_root, &walk_options);
        if files.is_empty() {
            return Ok(Exit::success());
        }

        // Prime shared intrinsic and Template-index work before the database is
        // cloned into Rayon workers.
        djls_ide::prepare_project_template_analysis(&db)
            .context("Failed to prepare project Template analysis")?;

        let results = check_files_parallel(db, files)?;
        report_results(results, &config, &fmt, quiet, input.summary())
    }
}

fn report_results(
    mut results: Vec<FileCheckResult>,
    config: &djls_conf::DiagnosticsConfig,
    fmt: &DiagnosticRenderer,
    quiet: bool,
    summary_style: SummaryStyle,
) -> Result<Exit> {
    results.sort_by(|left, right| left.path.cmp(&right.path));

    let mut error_count = 0;
    let mut file_count = 0;
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();

    for result in results {
        if quiet {
            let count = result.renderable_diagnostic_count(config);
            if count > 0 {
                file_count += 1;
                error_count += count;
            }
            continue;
        }

        let rendered = result.render(config, fmt);
        if rendered.is_empty() {
            continue;
        }

        file_count += 1;
        error_count += rendered.len();
        for output in rendered {
            writeln!(stdout, "{output}\n")?;
        }
    }

    if error_count == 0 {
        return Ok(Exit::success());
    }
    if quiet {
        return Ok(Exit::error());
    }

    let error_word = if error_count == 1 { "error" } else { "errors" };
    let message = match summary_style {
        SummaryStyle::Files => {
            let file_word = if file_count == 1 { "file" } else { "files" };
            format!("Found {error_count} {error_word} in {file_count} {file_word}.")
        }
        SummaryStyle::Stdin => format!("Found {error_count} {error_word}."),
    };
    Ok(Exit::error().with_message(message))
}

/// Validate paths with the same per-clone Rayon execution used by the batch CLI.
fn check_files_parallel(
    db: DjangoDatabase,
    files: Vec<Utf8PathBuf>,
) -> Result<Vec<FileCheckResult>> {
    // DjangoDatabase is Send + !Sync (salsa::Storage has RefCell). Clone the
    // already-primed database per task so validation cannot lazily become the
    // owner of shared intrinsic work.
    let (tx, rx) = std::sync::mpsc::channel();
    rayon::scope(move |scope| {
        for path in files {
            let db = db.clone();
            let tx = tx.clone();
            scope.spawn(move |_| match check_file_with_source(&db, &path) {
                Ok(result) if result.check.has_diagnostics() => {
                    let _ = tx.send(Ok(result));
                }
                Ok(_) => {}
                Err(error) => {
                    let _ = tx.send(Err(error));
                }
            });
        }
    });

    rx.into_iter().collect()
}

struct SingleFileOverlay {
    path: Utf8PathBuf,
    contents: String,
    disk: OsFileSystem,
}

impl SingleFileOverlay {
    fn new(path: Utf8PathBuf, contents: String, disk: OsFileSystem) -> Self {
        Self {
            path,
            contents,
            disk,
        }
    }
}

impl FileSystem for SingleFileOverlay {
    fn read_to_string(&self, path: &Utf8Path) -> std::io::Result<String> {
        if path == self.path {
            Ok(self.contents.clone())
        } else {
            self.disk.read_to_string(path)
        }
    }

    fn exists(&self, path: &Utf8Path) -> bool {
        path == self.path || self.disk.exists(path)
    }

    fn is_file(&self, path: &Utf8Path) -> bool {
        path == self.path || self.disk.is_file(path)
    }

    fn is_dir(&self, path: &Utf8Path) -> bool {
        self.disk.is_dir(path)
    }

    fn case_sensitivity(&self) -> CaseSensitivity {
        self.disk.case_sensitivity()
    }

    fn path_exists_case_sensitive(&self, path: &Utf8Path, prefix: &Utf8Path) -> bool {
        path == self.path || self.disk.path_exists_case_sensitive(path, prefix)
    }

    fn walk_root(&self, root: &Utf8Path, options: &WalkOptions) -> RootWalk {
        self.disk.walk_root(root, options)
    }
}

/// Run validation and capture the source text for later rendering.
fn check_file_with_source(db: &DjangoDatabase, path: &Utf8Path) -> Result<FileCheckResult> {
    let Ok(file) = path_to_file(db, path) else {
        return Ok(FileCheckResult {
            path: path.to_owned(),
            source: SourceText::default(),
            check: CheckResult {
                template_errors: Vec::new(),
                validation_errors: Vec::new(),
            },
        });
    };
    let source = file.try_source(db)?;
    let check = check_file(db, file);

    Ok(FileCheckResult {
        path: path.to_owned(),
        source,
        check,
    })
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

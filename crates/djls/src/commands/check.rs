use std::io::IsTerminal;
use std::io::Read as _;
use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use clap::Parser;
use djls_db::walk::walk_template_files;
use djls_db::DjangoDatabase;
use djls_semantic::Db as SemanticDb;
use djls_semantic::ValidationError;
use djls_semantic::ValidationErrorAccumulator;
use djls_source::Db as SourceDb;
use djls_source::Diagnostic;
use djls_source::DiagnosticRenderer;
use djls_source::Severity;
use djls_source::Span;
use djls_templates::TemplateError;
use djls_templates::TemplateErrorAccumulator;
use djls_workspace::OsFileSystem;

use crate::args::Args;
use crate::commands::Command;
use crate::exit::Exit;

#[derive(Debug, Parser)]
pub struct Check {
    /// Files or directories to check. If omitted, discovers template
    /// directories from the Django project.
    paths: Vec<Utf8PathBuf>,

    /// Select specific diagnostic codes to enable (e.g. S100,S101).
    #[arg(long, value_delimiter = ',')]
    select: Vec<String>,

    /// Ignore specific diagnostic codes (e.g. S108,S109).
    #[arg(long, value_delimiter = ',')]
    ignore: Vec<String>,
}

impl Command for Check {
    fn execute(&self, _args: &Args) -> Result<Exit> {
        let project_root = resolve_project_root()?;
        let settings =
            djls_conf::Settings::new(&project_root, None).context("Failed to load settings")?;

        let config = build_diagnostics_config(&settings, &self.select, &self.ignore);
        let fmt = pick_renderer();

        let reading_stdin = !std::io::stdin().is_terminal() && self.paths.is_empty();

        if reading_stdin {
            return check_stdin(&project_root, &settings, &config, &fmt);
        }

        let fs: Arc<dyn djls_workspace::FileSystem> = Arc::new(OsFileSystem);
        let db = DjangoDatabase::new(fs, &settings, Some(&project_root));

        let files = discover_files(&self.paths, &db, &project_root);

        if files.is_empty() {
            return Ok(Exit::success());
        }

        let mut error_count: usize = 0;
        let mut file_count: usize = 0;

        for path in &files {
            let file_diagnostics = check_file(&db, path, &config, &fmt);
            if !file_diagnostics.is_empty() {
                file_count += 1;
                for output in &file_diagnostics {
                    println!("{output}");
                }
                error_count += file_diagnostics.len();
            }
        }

        if error_count > 0 {
            let file_word = if file_count == 1 { "file" } else { "files" };
            let error_word = if error_count == 1 { "error" } else { "errors" };
            Ok(Exit::error().with_message(format!(
                "Found {error_count} {error_word} in {file_count} {file_word}."
            )))
        } else {
            Ok(Exit::success())
        }
    }
}

fn discover_files(
    paths: &[Utf8PathBuf],
    db: &DjangoDatabase,
    project_root: &Utf8Path,
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
        return walk_template_files(&resolved);
    }

    if let Some(dirs) = db.template_dirs() {
        let dirs: Vec<Utf8PathBuf> = dirs.into_iter().collect();
        walk_template_files(&dirs)
    } else {
        walk_template_files(&[project_root.to_owned()])
    }
}

fn check_stdin(
    project_root: &Utf8Path,
    settings: &djls_conf::Settings,
    config: &djls_conf::DiagnosticsConfig,
    fmt: &DiagnosticRenderer,
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

    let diagnostics = check_file(&db, &stdin_path, config, fmt);
    if diagnostics.is_empty() {
        Ok(Exit::success())
    } else {
        for output in &diagnostics {
            println!("{output}");
        }
        let count = diagnostics.len();
        let word = if count == 1 { "error" } else { "errors" };
        Ok(Exit::error().with_message(format!("Found {count} {word}.")))
    }
}

fn check_file(
    db: &DjangoDatabase,
    path: &Utf8Path,
    config: &djls_conf::DiagnosticsConfig,
    fmt: &DiagnosticRenderer,
) -> Vec<String> {
    let file = db.get_or_create_file(path);
    let source_text = file.source(db);
    let source = source_text.as_str();

    let nodelist = djls_templates::parse_template(db, file);

    let mut results = Vec::new();

    let template_errors =
        djls_templates::parse_template::accumulated::<TemplateErrorAccumulator>(db, file);
    for error_acc in &template_errors {
        if let Some(output) =
            render_template_error(source, path.as_str(), &error_acc.0, config, fmt)
        {
            results.push(output);
        }
    }

    if let Some(nodelist) = nodelist {
        djls_semantic::validate_nodelist(db, nodelist);

        let validation_errors = djls_semantic::validate_nodelist::accumulated::<
            ValidationErrorAccumulator,
        >(db, nodelist);

        let mut errors: Vec<&ValidationError> =
            validation_errors.iter().map(|acc| &acc.0).collect();
        errors.sort_by_key(|e| e.primary_span().map_or(0, Span::start));

        for err in errors {
            if let Some(output) = render_validation_error(source, path.as_str(), err, config, fmt) {
                results.push(output);
            }
        }
    }

    results
}

fn render_template_error(
    source: &str,
    path: &str,
    error: &TemplateError,
    config: &djls_conf::DiagnosticsConfig,
    fmt: &DiagnosticRenderer,
) -> Option<String> {
    let code = match error {
        TemplateError::Parser(_) => "T100",
        TemplateError::Io(_) => "T900",
        TemplateError::Config(_) => "T901",
    };

    let severity = config.get_severity(code);
    if severity == djls_conf::DiagnosticSeverity::Off {
        return None;
    }

    let message = error.to_string();
    let diag = Diagnostic::new(
        source,
        path,
        code,
        &message,
        to_render_severity(severity),
        Span::new(0, 0),
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

fn to_render_severity(severity: djls_conf::DiagnosticSeverity) -> Severity {
    match severity {
        djls_conf::DiagnosticSeverity::Error => Severity::Error,
        djls_conf::DiagnosticSeverity::Warning => Severity::Warning,
        djls_conf::DiagnosticSeverity::Info => Severity::Info,
        djls_conf::DiagnosticSeverity::Hint | djls_conf::DiagnosticSeverity::Off => Severity::Hint,
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

fn pick_renderer() -> DiagnosticRenderer {
    if std::io::stdout().is_terminal() {
        DiagnosticRenderer::styled()
    } else {
        DiagnosticRenderer::plain()
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::process::Command as ProcessCommand;

    fn djls_binary() -> std::path::PathBuf {
        let mut path = std::env::current_exe().unwrap();
        // test binary lives in target/debug/deps/djls-HASH
        // actual binary is target/debug/djls
        path.pop(); // remove the test binary name
        if path.ends_with("deps") {
            path.pop();
        }
        path.push("djls");
        path
    }

    fn setup_project(dir: &std::path::Path) {
        std::fs::write(
            dir.join("djls.toml"),
            r#"
[tagspecs]
version = "0.6.0"
engine = "django"

[[tagspecs.libraries]]
module = "django.template.defaulttags"

[[tagspecs.libraries.tags]]
name = "block"
type = "block"

[tagspecs.libraries.tags.end]
name = "endblock"

[[tagspecs.libraries.tags]]
name = "if"
type = "block"

[tagspecs.libraries.tags.end]
name = "endif"

[[tagspecs.libraries.tags]]
name = "for"
type = "block"

[tagspecs.libraries.tags.end]
name = "endfor"
"#,
        )
        .unwrap();
    }

    #[test]
    fn check_clean_template_exits_zero() {
        let dir = tempfile::tempdir().unwrap();
        setup_project(dir.path());

        let templates = dir.path().join("templates");
        std::fs::create_dir_all(&templates).unwrap();
        std::fs::write(
            templates.join("good.html"),
            "{% block content %}<p>Hello</p>{% endblock %}\n",
        )
        .unwrap();

        let output = ProcessCommand::new(djls_binary())
            .args(["check", "templates/"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "Expected exit 0, got {:?}\nstdout: {}\nstderr: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn check_broken_template_exits_one() {
        let dir = tempfile::tempdir().unwrap();
        setup_project(dir.path());

        let templates = dir.path().join("templates");
        std::fs::create_dir_all(&templates).unwrap();
        std::fs::write(
            templates.join("broken.html"),
            "{% block content %}\n<p>Hello</p>\n",
        )
        .unwrap();

        let output = ProcessCommand::new(djls_binary())
            .args(["check", "templates/"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        assert_eq!(output.status.code(), Some(1));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("S100"),
            "Expected S100 error code in output:\n{stdout}"
        );
        assert!(
            stdout.contains("Unclosed tag"),
            "Expected 'Unclosed tag' in output:\n{stdout}"
        );
    }

    #[test]
    fn check_ignore_suppresses_errors() {
        let dir = tempfile::tempdir().unwrap();
        setup_project(dir.path());

        let templates = dir.path().join("templates");
        std::fs::create_dir_all(&templates).unwrap();
        std::fs::write(
            templates.join("broken.html"),
            "{% block content %}\n<p>Hello</p>\n",
        )
        .unwrap();

        let output = ProcessCommand::new(djls_binary())
            .args(["check", "--ignore", "S100", "templates/"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "Expected exit 0 with --ignore S100, got {:?}\nstdout: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
        );
    }

    #[test]
    fn check_stdin_detects_errors() {
        let dir = tempfile::tempdir().unwrap();
        setup_project(dir.path());

        let mut child = ProcessCommand::new(djls_binary())
            .args(["check"])
            .current_dir(dir.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();

        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"{% block content %}<p>Hello</p>\n")
            .unwrap();

        let output = child.wait_with_output().unwrap();

        assert_eq!(output.status.code(), Some(1));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("S100"),
            "Expected S100 in stdin output:\n{stdout}"
        );
    }

    #[test]
    fn check_no_templates_exits_zero() {
        let dir = tempfile::tempdir().unwrap();
        setup_project(dir.path());

        let empty_dir = dir.path().join("templates");
        std::fs::create_dir_all(&empty_dir).unwrap();

        let output = ProcessCommand::new(djls_binary())
            .args(["check", "templates/"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "Expected exit 0 for empty dir, got {:?}",
            output.status.code(),
        );
    }
}

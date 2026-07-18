use camino::Utf8Path;
use camino::Utf8PathBuf;
use djls_conf::DiagnosticsConfig;
use djls_semantic::TemplateDiagnostics;
use djls_semantic::ValidationError;
use djls_source::Diagnostic;
use djls_source::DiagnosticRenderer;
use djls_source::File;
use djls_source::FileReadError;
use djls_source::Severity;
use djls_source::SourceText;
use djls_source::Span;
use djls_templates::TemplateError;

/// A readable Template and its collected diagnostics, ready for terminal output.
pub struct CheckedTemplate {
    path: Utf8PathBuf,
    source: SourceText,
    diagnostics: TemplateDiagnostics,
}

impl CheckedTemplate {
    /// Return the Template path used in rendered diagnostics.
    #[must_use]
    pub fn path(&self) -> &Utf8Path {
        &self.path
    }

    /// Return whether syntax or validation produced any diagnostics.
    #[must_use]
    pub fn has_diagnostics(&self) -> bool {
        self.diagnostics.has_diagnostics()
    }

    /// Count diagnostics enabled by `config` that can be rendered.
    #[must_use]
    pub fn renderable_diagnostic_count(&self, config: &DiagnosticsConfig) -> usize {
        self.diagnostics
            .template_errors
            .iter()
            .filter(|error| diagnostic_is_enabled(config, error.diagnostic_code()))
            .count()
            + self
                .diagnostics
                .validation_errors
                .iter()
                .filter(|error| {
                    diagnostic_is_enabled(config, error.code()) && error.primary_span().is_some()
                })
                .count()
    }

    /// Render enabled diagnostics in terminal output order.
    #[must_use]
    pub fn render(&self, config: &DiagnosticsConfig, fmt: &DiagnosticRenderer) -> Vec<String> {
        let mut results = Vec::with_capacity(self.renderable_diagnostic_count(config));
        let path = self.path.as_str();
        let source = self.source.as_str();

        for error in &self.diagnostics.template_errors {
            if let Some(output) = render_template_error(source, path, error, config, fmt) {
                results.push(output);
            }
        }

        for error in &self.diagnostics.validation_errors {
            if let Some(output) = render_validation_error(source, path, error, config, fmt) {
                results.push(output);
            }
        }

        results
    }
}

/// Read, validate, and collect one Template for terminal reporting.
pub fn check_template(
    db: &dyn djls_semantic::Db,
    file: File,
) -> Result<CheckedTemplate, FileReadError> {
    let source = file.try_source(db)?;
    let mut diagnostics = djls_semantic::collect_template_diagnostics(db, file);
    diagnostics
        .validation_errors
        .sort_by_cached_key(|error| error.primary_span().map_or(0, Span::start));

    Ok(CheckedTemplate {
        path: file.path(db).to_owned(),
        source,
        diagnostics,
    })
}

fn diagnostic_is_enabled(config: &DiagnosticsConfig, code: &str) -> bool {
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
    config: &DiagnosticsConfig,
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
    config: &DiagnosticsConfig,
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

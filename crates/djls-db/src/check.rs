use camino::Utf8PathBuf;
use djls_conf::DiagnosticsConfig;
use djls_semantic::Db as SemanticDb;
use djls_semantic::ValidationError;
use djls_semantic::ValidationErrorAccumulator;
use djls_source::Diagnostic;
use djls_source::DiagnosticRenderer;
use djls_source::File;
use djls_source::Severity;
use djls_source::Span;
use djls_templates::TemplateError;
use djls_templates::TemplateErrorAccumulator;

/// Raw diagnostic data from checking a single template file.
///
/// Contains both parse errors and semantic validation errors.
/// Produced by [`check_file`] and consumed by either CLI rendering
/// or LSP diagnostic conversion.
pub struct CheckResult {
    pub template_errors: Vec<TemplateError>,
    pub validation_errors: Vec<ValidationError>,
}

impl CheckResult {
    #[must_use]
    pub fn has_diagnostics(&self) -> bool {
        !self.template_errors.is_empty() || !self.validation_errors.is_empty()
    }
}

/// Check a single template file: parse, validate, and collect all errors.
///
/// This is the shared orchestration that both the CLI and LSP server
/// use to drive diagnostics. Under the hood it triggers Salsa-tracked
/// `parse_template` and `validate_nodelist` queries (cached across calls).
pub fn check_file(db: &dyn SemanticDb, file: File) -> CheckResult {
    let nodelist = djls_templates::parse_template(db, file);

    let template_errors: Vec<TemplateError> =
        djls_templates::parse_template::accumulated::<TemplateErrorAccumulator>(db, file)
            .iter()
            .map(|acc| acc.0.clone())
            .collect();

    let mut validation_errors: Vec<ValidationError> = Vec::new();

    if let Some(nodelist) = nodelist {
        let accumulated = djls_semantic::validate_nodelist::accumulated::<ValidationErrorAccumulator>(
            db, nodelist,
        );

        validation_errors = accumulated.iter().map(|acc| acc.0.clone()).collect();
        validation_errors.sort_by_key(|e| e.primary_span().map_or(0, Span::start));
    }

    CheckResult {
        template_errors,
        validation_errors,
    }
}

/// Per-file check result bundled with the source text and path needed
/// for rendering. Used by both the CLI and benchmarks.
pub struct FileCheckResult {
    pub path: Utf8PathBuf,
    pub source: String,
    pub check: CheckResult,
}

impl FileCheckResult {
    #[must_use]
    pub fn has_diagnostics(&self) -> bool {
        self.check.has_diagnostics()
    }

    #[must_use]
    pub fn render(&self, config: &DiagnosticsConfig, fmt: &DiagnosticRenderer) -> Vec<String> {
        let mut results = Vec::new();
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

// `Off` is never reached in practice — both `render_template_error` and
// `render_validation_error` early-return before calling this. Kept as a
// defensive fallback since the function signature accepts any severity.
fn to_render_severity(severity: djls_conf::DiagnosticSeverity) -> Severity {
    match severity {
        djls_conf::DiagnosticSeverity::Error => Severity::Error,
        djls_conf::DiagnosticSeverity::Warning => Severity::Warning,
        djls_conf::DiagnosticSeverity::Info => Severity::Info,
        djls_conf::DiagnosticSeverity::Hint | djls_conf::DiagnosticSeverity::Off => Severity::Hint,
    }
}

/// Render a template parse error to a formatted string.
///
/// Returns `None` if the diagnostic code is suppressed by `config`.
#[must_use]
pub fn render_template_error(
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
    // TemplateError loses position info during the ParseError → String
    // conversion, so we use a zero-span (file-level) diagnostic for now.
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

/// Render a semantic validation error to a formatted string.
///
/// Returns `None` if the diagnostic code is suppressed by `config`
/// or the error has no primary span.
#[must_use]
pub fn render_validation_error(
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

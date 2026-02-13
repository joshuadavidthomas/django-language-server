use std::cell::RefCell;
use std::collections::HashMap;

use djls_conf::DiagnosticsConfig;
use djls_semantic::ValidationError;
use djls_source::Diagnostic;
use djls_source::DiagnosticRenderer;
use djls_source::File;
use djls_source::LineIndex;
use djls_source::Severity;
use djls_source::Span;
use djls_templates::TemplateError;
use djls_templates::TemplateErrorAccumulator;
use tower_lsp_server::ls_types;

use crate::ext::DiagnosticSeverityExt;
use crate::ext::SpanExt;

const RULES_BASE_URL: &str = "https://djls.joshthomas.dev/rules/#";

fn rule_url(name: &'static str) -> ls_types::Uri {
    thread_local! {
        static CACHE: RefCell<HashMap<&'static str, ls_types::Uri>> =
            RefCell::new(HashMap::new());
    }

    CACHE.with(|cache| {
        cache
            .borrow_mut()
            .entry(name)
            .or_insert_with(|| {
                format!("{RULES_BASE_URL}{name}")
                    .parse()
                    .expect("valid docs URL")
            })
            .clone()
    })
}

trait DiagnosticError: std::fmt::Display {
    fn span(&self) -> Option<(u32, u32)>;
    fn diagnostic_code(&self) -> &'static str;
    fn diagnostic_name(&self) -> &'static str;

    fn message(&self) -> String {
        self.to_string()
    }

    fn as_diagnostic(&self, line_index: &LineIndex) -> ls_types::Diagnostic {
        let range = self
            .span()
            .map(|(start, length)| Span::new(start, length).to_lsp_range(line_index))
            .unwrap_or_default();

        ls_types::Diagnostic {
            range,
            severity: Some(ls_types::DiagnosticSeverity::ERROR),
            code: Some(ls_types::NumberOrString::String(
                self.diagnostic_code().to_string(),
            )),
            code_description: Some(ls_types::CodeDescription {
                href: rule_url(self.diagnostic_name()),
            }),
            source: Some(crate::SOURCE_NAME.to_string()),
            message: self.message(),
            related_information: None,
            tags: None,
            data: None,
        }
    }
}

impl DiagnosticError for TemplateError {
    fn span(&self) -> Option<(u32, u32)> {
        None
    }

    fn diagnostic_code(&self) -> &'static str {
        match self {
            TemplateError::Parser(_) => "T100",
            TemplateError::Io(_) => "T900",
            TemplateError::Config(_) => "T901",
        }
    }

    fn diagnostic_name(&self) -> &'static str {
        self.name()
    }
}

impl DiagnosticError for ValidationError {
    fn span(&self) -> Option<(u32, u32)> {
        self.primary_span().map(Into::into)
    }

    fn diagnostic_code(&self) -> &'static str {
        self.code()
    }

    fn diagnostic_name(&self) -> &'static str {
        self.name()
    }
}

fn push_with_severity(
    mut diagnostic: ls_types::Diagnostic,
    config: &djls_conf::diagnostics::DiagnosticsConfig,
    diagnostics: &mut Vec<ls_types::Diagnostic>,
) {
    if let Some(ls_types::NumberOrString::String(code)) = &diagnostic.code {
        let severity = config.get_severity(code);

        if let Some(lsp_severity) = severity.to_lsp_severity() {
            diagnostic.severity = Some(lsp_severity);
            diagnostics.push(diagnostic);
        }
    } else {
        diagnostics.push(diagnostic);
    }
}

/// Collect all diagnostics for a template file.
///
/// This function collects and converts errors that were accumulated during
/// parsing and validation. The caller must provide the parsed `NodeList` (or `None`
/// if parsing failed), making it explicit that parsing should have already occurred.
///
/// Diagnostics are filtered based on the configuration settings (`select` and `ignore`),
/// and severity levels can be overridden per diagnostic code.
///
/// # Parameters
/// - `db`: The Salsa database
/// - `file`: The source file (needed to retrieve accumulated template errors)
/// - `nodelist`: The parsed AST, or None if parsing failed
///
/// # Returns
/// A vector of LSP diagnostics combining both template syntax errors and
/// semantic validation errors, filtered by the diagnostics configuration.
///
/// # Design
/// This API design makes it clear that:
/// - Parsing must happen before collecting diagnostics
/// - This function only collects and converts existing errors
/// - The `NodeList` provides both line offsets and access to validation errors
#[must_use]
pub fn collect_diagnostics(
    db: &dyn djls_semantic::Db,
    file: File,
    nodelist: Option<djls_templates::NodeList<'_>>,
) -> Vec<ls_types::Diagnostic> {
    let mut diagnostics = Vec::new();

    let config = db.diagnostics_config();

    let template_errors =
        djls_templates::parse_template::accumulated::<TemplateErrorAccumulator>(db, file);

    let line_index = file.line_index(db);

    for error_acc in template_errors {
        let diagnostic = error_acc.0.as_diagnostic(line_index);
        push_with_severity(diagnostic, &config, &mut diagnostics);
    }

    if let Some(nodelist) = nodelist {
        let validation_errors = djls_semantic::validate_nodelist::accumulated::<
            djls_semantic::ValidationErrorAccumulator,
        >(db, nodelist);

        for error_acc in validation_errors {
            let diagnostic = error_acc.0.as_diagnostic(line_index);
            push_with_severity(diagnostic, &config, &mut diagnostics);
        }
    }

    diagnostics
}

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

#[cfg(test)]
mod tests {
    use djls_conf::DiagnosticSeverity;

    use super::*;

    #[test]
    fn test_to_lsp_severity() {
        assert_eq!(DiagnosticSeverity::Off.to_lsp_severity(), None);
        assert_eq!(
            DiagnosticSeverity::Error.to_lsp_severity(),
            Some(ls_types::DiagnosticSeverity::ERROR)
        );
        assert_eq!(
            DiagnosticSeverity::Warning.to_lsp_severity(),
            Some(ls_types::DiagnosticSeverity::WARNING)
        );
        assert_eq!(
            DiagnosticSeverity::Info.to_lsp_severity(),
            Some(ls_types::DiagnosticSeverity::INFORMATION)
        );
        assert_eq!(
            DiagnosticSeverity::Hint.to_lsp_severity(),
            Some(ls_types::DiagnosticSeverity::HINT)
        );
    }
}

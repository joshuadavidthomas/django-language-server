use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use djls_conf::DiagnosticsConfig;
use djls_semantic::ValidationError;
use djls_semantic::ValidationErrorAccumulator;
use djls_source::Diagnostic;
use djls_source::DiagnosticRenderer;
use djls_source::File;
use djls_source::Severity;
use djls_source::SourceText;
use djls_source::Span;
use djls_templates::TemplateError;
use djls_templates::TemplateErrorAccumulator;

pub struct CheckResult {
    template_errors: Vec<TemplateError>,
    pub validation_errors: Vec<ValidationError>,
}

/// Stable semantic output contract for a checked set of templates.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiagnosticDigest {
    parser_count: usize,
    validation_count: usize,
    codes: BTreeMap<&'static str, usize>,
}

impl DiagnosticDigest {
    #[must_use]
    pub fn from_counts(
        parser_count: usize,
        validation_count: usize,
        codes: impl IntoIterator<Item = (&'static str, usize)>,
    ) -> Self {
        Self {
            parser_count,
            validation_count,
            codes: codes.into_iter().collect(),
        }
    }

    #[must_use]
    pub fn total(&self) -> usize {
        self.parser_count + self.validation_count
    }

    pub fn merge(&mut self, other: &Self) {
        self.parser_count += other.parser_count;
        self.validation_count += other.validation_count;
        for (&code, &count) in &other.codes {
            *self.codes.entry(code).or_default() += count;
        }
    }
}

impl CheckResult {
    #[must_use]
    fn has_diagnostics(&self) -> bool {
        !self.template_errors.is_empty() || !self.validation_errors.is_empty()
    }

    #[must_use]
    pub fn diagnostic_digest(&self) -> DiagnosticDigest {
        let mut codes = BTreeMap::new();
        for error in &self.template_errors {
            *codes.entry(error.diagnostic_code()).or_default() += 1;
        }
        for error in &self.validation_errors {
            *codes.entry(error.code()).or_default() += 1;
        }

        DiagnosticDigest {
            parser_count: self.template_errors.len(),
            validation_count: self.validation_errors.len(),
            codes,
        }
    }
}

pub struct FileCheckResult {
    pub path: Utf8PathBuf,
    pub source: SourceText,
    pub check: CheckResult,
}

impl FileCheckResult {
    #[must_use]
    pub fn has_diagnostics(&self) -> bool {
        self.check.has_diagnostics()
    }

    #[must_use]
    pub fn diagnostic_digest(&self) -> DiagnosticDigest {
        self.check.diagnostic_digest()
    }

    #[must_use]
    fn renderable_diagnostic_count(&self, config: &DiagnosticsConfig) -> usize {
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

    #[must_use]
    pub fn render(&self, config: &DiagnosticsConfig, fmt: &DiagnosticRenderer) -> Vec<String> {
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

#[must_use]
pub fn check_file(db: &dyn djls_semantic::Db, file: File) -> CheckResult {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::realistic_db;
    use crate::template_fixtures;
    use crate::validation_error_fixtures;

    fn fixture_digest(fixtures: &[crate::Fixture]) -> DiagnosticDigest {
        let mut db = realistic_db();
        let mut digest = DiagnosticDigest::default();
        for fixture in fixtures {
            let file = db.file_with_contents(fixture.path.clone(), &fixture.source);
            digest.merge(&check_file(&db, file).diagnostic_digest());
        }
        digest
    }

    #[test]
    fn template_fixture_diagnostics_are_stable() {
        assert_eq!(
            fixture_digest(template_fixtures()),
            DiagnosticDigest::from_counts(0, 87, [("S108", 22), ("S109", 45), ("S111", 20)],)
        );
    }

    #[test]
    fn validation_error_fixture_diagnostics_are_stable() {
        assert_eq!(
            fixture_digest(validation_error_fixtures()),
            DiagnosticDigest::from_counts(
                0,
                751,
                [
                    ("S108", 253),
                    ("S109", 1),
                    ("S111", 123),
                    ("S115", 128),
                    ("S116", 124),
                    ("S117", 122),
                ],
            )
        );
    }
}

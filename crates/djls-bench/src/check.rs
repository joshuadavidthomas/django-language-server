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
    use std::sync::Arc;
    use std::sync::Mutex;

    use salsa::Database as _;

    use super::*;
    use crate::realistic_db;
    use crate::specs::realistic_db_with_event_log;
    use crate::template_fixtures;
    use crate::validation_error_fixtures;

    fn take_will_execute_names(
        db: &crate::Db,
        events: &Arc<Mutex<Vec<salsa::Event>>>,
    ) -> Vec<String> {
        std::mem::take(
            &mut *events
                .lock()
                .expect("benchmark event log lock should not be poisoned"),
        )
        .into_iter()
        .filter_map(|event| match event.kind {
            salsa::EventKind::WillExecute { database_key } => Some(
                db.ingredient_debug_name(database_key.ingredient_index())
                    .to_string(),
            ),
            _ => None,
        })
        .collect()
    }

    fn execution_count(names: &[String], query: &str) -> usize {
        names
            .iter()
            .filter(|name| name.rsplit("::").next() == Some(query))
            .count()
    }

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
    fn check_preparation_orders_shared_work_and_kernel_reuses_it() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut db = realistic_db_with_event_log(Arc::clone(&events));
        let files = template_fixtures()
            .iter()
            .map(|fixture| db.file_with_contents(fixture.path.clone(), &fixture.source))
            .collect::<Vec<_>>();

        djls_ide::prepare_project_template_analysis(&db)
            .expect("check benchmark database should install a Project");
        let setup_names = take_will_execute_names(&db, &events);
        let intrinsic_position = setup_names
            .iter()
            .position(|name| name.rsplit("::").next() == Some("semantic_grammar_vocabulary"))
            .expect("intrinsic priming should evaluate the semantic grammar");
        let template_index_position = setup_names
            .iter()
            .position(|name| name.rsplit("::").next() == Some("template_directory_index"))
            .expect("Template indexing should evaluate the directory index");
        assert!(
            intrinsic_position < template_index_position,
            "intrinsic products must be primed before Template indexing"
        );
        assert_eq!(execution_count(&setup_names, "template_directory_index"), 1);

        djls_ide::prepare_project_template_analysis(&db)
            .expect("check benchmark database should install a Project");
        let repeated_setup_names = take_will_execute_names(&db, &events);
        assert_eq!(
            execution_count(&repeated_setup_names, "semantic_grammar_vocabulary"),
            0,
            "repeated preparation should reuse intrinsic products"
        );
        assert_eq!(
            execution_count(&repeated_setup_names, "template_directory_index"),
            0,
            "repeated preparation should reuse the shared Template index"
        );

        let config = DiagnosticsConfig::default();
        let renderer = DiagnosticRenderer::plain();
        for file in &files {
            let source = file
                .try_source(&db)
                .expect("benchmark file should be readable");
            let path = file.path(&db).clone();
            let check = check_file(&db, *file);
            let result = FileCheckResult {
                path,
                source,
                check,
            };
            let _ = result.render(&config, &renderer);
        }

        let kernel_names = take_will_execute_names(&db, &events);
        assert_eq!(
            execution_count(&kernel_names, "template_directory_index"),
            0,
            "Template discovery/indexing must remain outside the timed check kernel"
        );
        assert_eq!(
            execution_count(&kernel_names, "validate_template_file"),
            files.len()
        );
    }

    #[test]
    fn template_fixture_diagnostics_are_stable() {
        // Canonical builtin identities intentionally remove unknown/unloaded output that the old
        // synthetic module identities produced. Fixture count and bytes remain unchanged.
        assert_eq!(
            fixture_digest(template_fixtures()),
            DiagnosticDigest::from_counts(0, 20, [("S111", 20)])
        );
    }

    #[test]
    fn validation_error_fixture_diagnostics_are_stable() {
        // Canonical loader and block roles intentionally expose the fixture's structural and `if`
        // expression errors instead of misclassifying their tags. Input cardinality is unchanged.
        assert_eq!(
            fixture_digest(validation_error_fixtures()),
            DiagnosticDigest::from_counts(
                0,
                873,
                [
                    ("S103", 127),
                    ("S108", 126),
                    ("S111", 123),
                    ("S114", 123),
                    ("S115", 128),
                    ("S116", 124),
                    ("S117", 122),
                ],
            )
        );
    }
}

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

pub const MANY_ERRORS_SOURCE: &str = concat!(
    "{% if and x %}oops{% endif %}\n",
    "{{ name|title:\"arg\" }}\n",
    "{{ text|truncatewords }}\n",
    "{% trans \"hello\" %}\n",
    "{% unknown_tag %}\n",
    "{% for %}empty{% endfor %}\n",
    "{{ value|bogus }}\n",
    "{% block a %}{% endblock b %}\n",
);

const SINGLE_SPAN_SOURCE: &str = "{% block content %}\n<p>Hello</p>\n{% endblock %}\n";
const MULTI_SPAN_SOURCE: &str = "{% block sidebar %}\n<nav>Links</nav>\n{% endblock content %}\n";

fn span_of(source: &str, needle: &str) -> Span {
    let start = source
        .find(needle)
        .expect("synthetic diagnostic span should exist in source");
    Span::saturating_from_bounds_usize(start, start + needle.len())
}

#[must_use]
pub fn synthetic_render_diagnostics() -> [Diagnostic<'static>; 2] {
    [
        Diagnostic::new(
            SINGLE_SPAN_SOURCE,
            "templates/page.html",
            "S100",
            "Unclosed tag: block",
            Severity::Error,
            span_of(SINGLE_SPAN_SOURCE, "{% block content %}"),
            "this block tag is never closed",
        ),
        Diagnostic::new(
            MULTI_SPAN_SOURCE,
            "templates/layout.html",
            "S103",
            "'content' does not match 'sidebar'",
            Severity::Error,
            span_of(MULTI_SPAN_SOURCE, "{% endblock content %}"),
            "closing tag says 'content'",
        )
        .annotation(
            span_of(MULTI_SPAN_SOURCE, "{% block sidebar %}"),
            "opening tag is 'sidebar'",
            false,
        ),
    ]
}

impl CheckResult {
    #[must_use]
    fn has_diagnostics(&self) -> bool {
        !self.template_errors.is_empty() || !self.validation_errors.is_empty()
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
    use std::collections::BTreeMap;
    use std::fmt::Write as _;
    use std::sync::Arc;
    use std::sync::Mutex;

    use salsa::Database as _;
    use serde::Serialize;
    use sha2::Digest;
    use sha2::Sha256;
    use tower_lsp_server::ls_types::DiagnosticSeverity;
    use tower_lsp_server::ls_types::NumberOrString;

    use super::*;
    use crate::CorpusTemplates;
    use crate::django_corpus_templates;
    use crate::full_corpus_templates;
    use crate::primed_realistic_db;
    use crate::realistic_db;
    use crate::specs::realistic_db_with_event_log;
    use crate::structure_db;
    use crate::template_fixtures;
    use crate::validation_error_fixtures;

    struct CheckDigest {
        parser_count: usize,
        validation_count: usize,
        codes: BTreeMap<&'static str, usize>,
    }

    impl CheckDigest {
        fn from_check(check: &CheckResult) -> Self {
            let mut codes = BTreeMap::new();
            for error in &check.template_errors {
                *codes.entry(error.diagnostic_code()).or_default() += 1;
            }
            for error in &check.validation_errors {
                *codes.entry(error.code()).or_default() += 1;
            }

            Self {
                parser_count: check.template_errors.len(),
                validation_count: check.validation_errors.len(),
                codes,
            }
        }

        fn total(&self) -> usize {
            self.parser_count + self.validation_count
        }
    }

    struct StableHasher(Sha256);

    impl StableHasher {
        fn new() -> Self {
            Self(Sha256::new())
        }

        fn write(&mut self, bytes: &[u8]) {
            self.0.update(
                u64::try_from(bytes.len())
                    .expect("benchmark snapshot record should fit in u64")
                    .to_le_bytes(),
            );
            self.0.update(bytes);
        }

        fn finish(self) -> String {
            hex(&self.0.finalize())
        }
    }

    fn hex(bytes: &[u8]) -> String {
        let mut output = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            write!(output, "{byte:02x}").expect("writing to a String should not fail");
        }
        output
    }

    fn sha256(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    #[derive(Serialize)]
    struct CorpusInputSnapshot {
        discovered_file_count: usize,
        synchronized_file_count: usize,
        total_bytes: usize,
        sorted_records_sha256: String,
    }

    fn corpus_input_snapshot(corpus: &CorpusTemplates) -> CorpusInputSnapshot {
        let mut records: Vec<_> = corpus.files.iter().collect();
        records.sort_by(|left, right| left.0.cmp(&right.0));
        let mut hasher = StableHasher::new();
        for (path, source) in records {
            hasher.write(path.as_str().as_bytes());
            hasher.write(
                &u64::try_from(source.len())
                    .expect("corpus template size should fit in u64")
                    .to_le_bytes(),
            );
            hasher.write(source.as_bytes());
        }

        CorpusInputSnapshot {
            discovered_file_count: corpus.discovered_file_count,
            synchronized_file_count: corpus.files.len(),
            total_bytes: corpus.files.iter().map(|(_, source)| source.len()).sum(),
            sorted_records_sha256: hasher.finish(),
        }
    }

    #[derive(Clone, Debug, Ord, PartialOrd, Eq, PartialEq, Serialize)]
    struct NormalizedDiagnostic {
        path: String,
        code: String,
        severity: String,
        start_line: u32,
        start_character: u32,
        end_line: u32,
        end_character: u32,
        message: String,
    }

    fn diagnostic_code(code: Option<NumberOrString>) -> String {
        match code {
            Some(NumberOrString::Number(code)) => code.to_string(),
            Some(NumberOrString::String(code)) => code,
            None => "none".to_string(),
        }
    }

    fn diagnostic_severity(severity: Option<DiagnosticSeverity>) -> String {
        match severity {
            Some(DiagnosticSeverity::ERROR) => "error".to_string(),
            Some(DiagnosticSeverity::WARNING) => "warning".to_string(),
            Some(DiagnosticSeverity::INFORMATION) => "information".to_string(),
            Some(DiagnosticSeverity::HINT) => "hint".to_string(),
            Some(severity) => format!("unknown({severity:?})"),
            None => "none".to_string(),
        }
    }

    fn hash_normalized_diagnostics(diagnostics: &[NormalizedDiagnostic]) -> String {
        let mut hasher = StableHasher::new();
        for diagnostic in diagnostics {
            hasher.write(diagnostic.path.as_bytes());
            hasher.write(diagnostic.code.as_bytes());
            hasher.write(diagnostic.severity.as_bytes());
            for value in [
                diagnostic.start_line,
                diagnostic.start_character,
                diagnostic.end_line,
                diagnostic.end_character,
            ] {
                hasher.write(&value.to_le_bytes());
            }
            hasher.write(diagnostic.message.as_bytes());
        }
        hasher.finish()
    }

    fn diagnostic_code_counts(diagnostics: &[NormalizedDiagnostic]) -> BTreeMap<String, usize> {
        let mut counts = BTreeMap::new();
        for diagnostic in diagnostics {
            *counts.entry(diagnostic.code.clone()).or_default() += 1;
        }
        counts
    }

    #[derive(Serialize)]
    struct PerFileCheckSnapshot {
        path: String,
        parser_count: usize,
        validation_count: usize,
        codes: BTreeMap<String, usize>,
    }

    fn hash_file_checks(checks: &[PerFileCheckSnapshot]) -> String {
        let mut hasher = StableHasher::new();
        for check in checks {
            hasher.write(check.path.as_bytes());
            hasher.write(
                &u64::try_from(check.parser_count)
                    .expect("parser count should fit in u64")
                    .to_le_bytes(),
            );
            hasher.write(
                &u64::try_from(check.validation_count)
                    .expect("validation count should fit in u64")
                    .to_le_bytes(),
            );
            for (code, count) in &check.codes {
                hasher.write(code.as_bytes());
                hasher.write(
                    &u64::try_from(*count)
                        .expect("diagnostic count should fit in u64")
                        .to_le_bytes(),
                );
            }
        }
        hasher.finish()
    }

    fn hash_paths(paths: &[String]) -> String {
        let mut hasher = StableHasher::new();
        for path in paths {
            hasher.write(path.as_bytes());
        }
        hasher.finish()
    }

    #[derive(Serialize)]
    struct RenderedOutputSnapshot {
        item_count: usize,
        total_bytes: usize,
        output_sha256: String,
    }

    #[derive(Clone, Copy)]
    enum WorkloadDetail {
        Small,
        Dense,
        Corpus,
    }

    impl WorkloadDetail {
        fn include_per_file_checks(self) -> bool {
            match self {
                Self::Small | Self::Dense => true,
                Self::Corpus => false,
            }
        }

        fn include_normalized_diagnostics(self) -> bool {
            match self {
                Self::Small => true,
                Self::Dense | Self::Corpus => false,
            }
        }

        fn include_diagnostic_bearing_paths(self) -> bool {
            match self {
                Self::Small | Self::Dense => true,
                Self::Corpus => false,
            }
        }
    }

    #[derive(Serialize)]
    struct CheckedWorkloadSnapshot {
        check_parser_count: usize,
        check_validation_count: usize,
        check_code_counts: BTreeMap<String, usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        per_file_checks: Option<Vec<PerFileCheckSnapshot>>,
        per_file_checks_sha256: String,
        ide_eligible_file_count: usize,
        ide_diagnostic_count: usize,
        ide_code_counts: BTreeMap<String, usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        normalized_ide_diagnostics: Option<Vec<NormalizedDiagnostic>>,
        normalized_ide_diagnostics_sha256: String,
        files_with_diagnostics: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        diagnostic_bearing_paths: Option<Vec<String>>,
        diagnostic_bearing_paths_sha256: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        rendered: Option<RenderedOutputSnapshot>,
    }

    fn checked_workload_snapshot(
        db: &crate::Db,
        files: &[File],
        detail: WorkloadDetail,
        render: bool,
    ) -> CheckedWorkloadSnapshot {
        let config = DiagnosticsConfig::default();
        let renderer = DiagnosticRenderer::plain();
        let mut parser_count = 0;
        let mut validation_count = 0;
        let mut check_code_counts = BTreeMap::new();
        let mut per_file_checks = Vec::new();
        let mut ide_eligible_file_count = 0;
        let mut normalized_ide_diagnostics = Vec::new();
        let mut diagnostic_bearing_paths = Vec::new();
        let mut rendered_count = 0;
        let mut rendered_bytes = 0;
        let mut rendered_hasher = StableHasher::new();

        for &file in files {
            let source = file
                .try_source(db)
                .expect("benchmark snapshot file should be readable");
            let path = file.path(db).clone();
            let check = check_file(db, file);
            let digest = CheckDigest::from_check(&check);
            parser_count += digest.parser_count;
            validation_count += digest.validation_count;
            for (&code, &count) in &digest.codes {
                *check_code_counts.entry(code.to_string()).or_default() += count;
            }
            if digest.total() > 0 {
                diagnostic_bearing_paths.push(path.to_string());
            }
            per_file_checks.push(PerFileCheckSnapshot {
                path: path.to_string(),
                parser_count: digest.parser_count,
                validation_count: digest.validation_count,
                codes: digest
                    .codes
                    .iter()
                    .map(|(&code, &count)| (code.to_string(), count))
                    .collect(),
            });

            if let Some(diagnostics) = djls_ide::collect_diagnostics(db, file) {
                ide_eligible_file_count += 1;
                normalized_ide_diagnostics.extend(diagnostics.into_iter().map(|diagnostic| {
                    NormalizedDiagnostic {
                        path: path.to_string(),
                        code: diagnostic_code(diagnostic.code),
                        severity: diagnostic_severity(diagnostic.severity),
                        start_line: diagnostic.range.start.line,
                        start_character: diagnostic.range.start.character,
                        end_line: diagnostic.range.end.line,
                        end_character: diagnostic.range.end.character,
                        message: diagnostic.message,
                    }
                }));
            }

            if render {
                let result = FileCheckResult {
                    path: path.clone(),
                    source,
                    check,
                };
                for output in result.render(&config, &renderer) {
                    rendered_count += 1;
                    rendered_bytes += output.len();
                    rendered_hasher.write(path.as_str().as_bytes());
                    rendered_hasher.write(output.as_bytes());
                }
            }
        }

        per_file_checks.sort_by(|left, right| left.path.cmp(&right.path));
        normalized_ide_diagnostics.sort();
        diagnostic_bearing_paths.sort();

        let ide_code_counts = diagnostic_code_counts(&normalized_ide_diagnostics);
        let per_file_checks_sha256 = hash_file_checks(&per_file_checks);
        let normalized_ide_diagnostics_sha256 =
            hash_normalized_diagnostics(&normalized_ide_diagnostics);
        let diagnostic_bearing_paths_sha256 = hash_paths(&diagnostic_bearing_paths);
        CheckedWorkloadSnapshot {
            check_parser_count: parser_count,
            check_validation_count: validation_count,
            check_code_counts,
            per_file_checks: detail.include_per_file_checks().then_some(per_file_checks),
            per_file_checks_sha256,
            ide_eligible_file_count,
            ide_diagnostic_count: normalized_ide_diagnostics.len(),
            ide_code_counts,
            normalized_ide_diagnostics: detail
                .include_normalized_diagnostics()
                .then_some(normalized_ide_diagnostics),
            normalized_ide_diagnostics_sha256,
            files_with_diagnostics: diagnostic_bearing_paths.len(),
            diagnostic_bearing_paths: detail
                .include_diagnostic_bearing_paths()
                .then_some(diagnostic_bearing_paths),
            diagnostic_bearing_paths_sha256,
            rendered: render.then(|| RenderedOutputSnapshot {
                item_count: rendered_count,
                total_bytes: rendered_bytes,
                output_sha256: rendered_hasher.finish(),
            }),
        }
    }

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
    fn fixture_check_workload_is_stable() {
        let mut db = realistic_db();
        let files = template_fixtures()
            .iter()
            .map(|fixture| db.file_with_contents(fixture.path.clone(), &fixture.source))
            .collect::<Vec<_>>();
        djls_ide::prepare_project_template_analysis(&db)
            .expect("check benchmark database should install a Project");

        insta::assert_yaml_snapshot!(
            "check_workload_fixtures",
            checked_workload_snapshot(&db, &files, WorkloadDetail::Small, true)
        );
    }

    #[derive(Serialize)]
    struct SemanticFileSnapshot {
        path: String,
        template_tree_regions: usize,
        has_opaque_regions: bool,
    }

    #[derive(Serialize)]
    struct SemanticWorkloadSnapshot {
        files: Vec<SemanticFileSnapshot>,
        total_template_tree_regions: usize,
        opaque_region_paths: Vec<String>,
    }

    #[test]
    fn projectless_semantic_workload_is_stable() {
        let mut db = structure_db();
        let files = template_fixtures()
            .iter()
            .map(|fixture| db.file_with_contents(fixture.path.clone(), &fixture.source))
            .collect::<Vec<_>>();
        let mut snapshots = Vec::new();
        for file in files {
            let nodelist =
                djls_templates::parse_template(&db, file).expect("benchmark template should parse");
            let tree = djls_semantic::build_template_tree_for_file(&db, file, nodelist);
            snapshots.push(SemanticFileSnapshot {
                path: file.path(&db).to_string(),
                template_tree_regions: tree.regions(&db).iter().count(),
                has_opaque_regions: !djls_semantic::compute_opaque_regions(&db, file, nodelist)
                    .is_empty(),
            });
        }
        snapshots.sort_by(|left, right| left.path.cmp(&right.path));
        let total_template_tree_regions = snapshots
            .iter()
            .map(|snapshot| snapshot.template_tree_regions)
            .sum();
        let opaque_region_paths = snapshots
            .iter()
            .filter(|snapshot| snapshot.has_opaque_regions)
            .map(|snapshot| snapshot.path.clone())
            .collect();

        insta::assert_yaml_snapshot!(
            "semantic_projectless_fixtures",
            SemanticWorkloadSnapshot {
                files: snapshots,
                total_template_tree_regions,
                opaque_region_paths,
            }
        );
    }

    #[derive(Serialize)]
    struct CorpusWorkloadSnapshot {
        input: CorpusInputSnapshot,
        output: CheckedWorkloadSnapshot,
    }

    fn snapshot_corpus(name: &str, corpus: Option<&CorpusTemplates>) {
        let Some(corpus) = corpus else {
            assert!(
                std::env::var_os("DJLS_REQUIRE_BENCH_CORPUS").is_none(),
                "{name} benchmark corpus is not synchronized"
            );
            eprintln!("{name} benchmark corpus is not synchronized; skipping snapshot");
            return;
        };
        assert_eq!(corpus.discovered_file_count, corpus.files.len());

        let mut db = realistic_db();
        let files = corpus
            .files
            .iter()
            .map(|(path, source)| db.file_with_contents(path.clone(), source))
            .collect::<Vec<_>>();
        djls_ide::prepare_project_template_analysis(&db)
            .expect("check benchmark database should install a Project");
        insta::assert_yaml_snapshot!(
            name,
            CorpusWorkloadSnapshot {
                input: corpus_input_snapshot(corpus),
                output: checked_workload_snapshot(&db, &files, WorkloadDetail::Corpus, true),
            }
        );
    }

    #[test]
    fn django_corpus_check_workload_is_stable() {
        let corpus = django_corpus_templates()
            .unwrap_or_else(|error| panic!("failed to load Django corpus templates: {error}"));
        snapshot_corpus("check_workload_corpus_django", corpus);
    }

    #[test]
    fn full_corpus_check_workload_is_stable() {
        let corpus = full_corpus_templates()
            .unwrap_or_else(|error| panic!("failed to load full corpus templates: {error}"));
        snapshot_corpus("check_workload_corpus_all", corpus);
    }

    #[test]
    fn cached_empty_diagnostics_workload_is_stable() {
        const EMPTY_FILE_COUNT: usize = 6;

        let mut db = primed_realistic_db();
        let files: Vec<_> = (0..EMPTY_FILE_COUNT)
            .map(|index| db.file_with_contents(format!("/templates/empty/{index}.html"), ""))
            .collect();

        insta::assert_yaml_snapshot!(
            "diagnostics_cached_empty",
            checked_workload_snapshot(&db, &files, WorkloadDetail::Small, false)
        );
    }

    #[test]
    fn cached_validation_errors_diagnostics_workload_is_stable() {
        let mut db = primed_realistic_db();
        let files: Vec<_> = validation_error_fixtures()
            .iter()
            .map(|fixture| db.file_with_contents(fixture.path.clone(), &fixture.source))
            .collect();

        insta::assert_yaml_snapshot!(
            "diagnostics_cached_validation_errors",
            checked_workload_snapshot(&db, &files, WorkloadDetail::Dense, false)
        );
    }

    #[test]
    fn cached_realistic_diagnostics_workload_is_stable() {
        let mut db = primed_realistic_db();
        let files: Vec<_> = template_fixtures()
            .iter()
            .map(|fixture| db.file_with_contents(fixture.path.clone(), &fixture.source))
            .collect();

        insta::assert_yaml_snapshot!(
            "diagnostics_cached_realistic_end_to_end",
            checked_workload_snapshot(&db, &files, WorkloadDetail::Small, false)
        );
    }

    #[derive(Serialize)]
    struct NamedRenderSnapshot {
        name: String,
        bytes: usize,
        output_sha256: String,
    }

    #[test]
    fn synthetic_rendering_workload_is_stable() {
        let plain = DiagnosticRenderer::plain();
        let styled = DiagnosticRenderer::styled();
        let diagnostics = synthetic_render_diagnostics();
        let outputs = [
            ("single_plain", plain.render(&diagnostics[0])),
            ("multi_plain", plain.render(&diagnostics[1])),
            ("single_styled", styled.render(&diagnostics[0])),
        ];
        let snapshot: Vec<_> = outputs
            .into_iter()
            .map(|(name, output)| NamedRenderSnapshot {
                name: name.to_string(),
                bytes: output.len(),
                output_sha256: sha256(output.as_bytes()),
            })
            .collect();

        insta::assert_yaml_snapshot!("diagnostics_render_synthetic", snapshot);
    }

    #[derive(Serialize)]
    struct ValidationRenderSnapshot {
        validation_error_count: usize,
        rendered_count: usize,
        rendered_bytes: usize,
        rendered_output_sha256: String,
    }

    #[test]
    fn validation_fixture_rendering_workload_is_stable() {
        let config = DiagnosticsConfig::default();
        let renderer = DiagnosticRenderer::plain();
        let mut validation_error_count = 0;
        let mut rendered_count = 0;
        let mut rendered_bytes = 0;
        let mut hasher = StableHasher::new();

        for fixture in validation_error_fixtures() {
            let mut db = primed_realistic_db();
            let file = db.file_with_contents(fixture.path.clone(), &fixture.source);
            let check = check_file(&db, file);
            assert!(
                !check.validation_errors.is_empty(),
                "validation error rendering fixture '{}' produced no validation errors",
                fixture.label,
            );
            validation_error_count += check.validation_errors.len();
            for error in &check.validation_errors {
                if let Some(output) = render_validation_error(
                    &fixture.source,
                    fixture.path.as_str(),
                    error,
                    &config,
                    &renderer,
                ) {
                    rendered_count += 1;
                    rendered_bytes += output.len();
                    hasher.write(fixture.path.as_str().as_bytes());
                    hasher.write(output.as_bytes());
                }
            }
        }

        insta::assert_yaml_snapshot!(
            "diagnostics_render_validation_fixtures",
            ValidationRenderSnapshot {
                validation_error_count,
                rendered_count,
                rendered_bytes,
                rendered_output_sha256: hasher.finish(),
            }
        );
    }

    #[derive(Serialize)]
    struct SyntheticValidationRenderSnapshot {
        source_bytes: usize,
        source_sha256: String,
        validation_error_count: usize,
        rendered_count_per_inner_iteration: usize,
        rendered_bytes_per_inner_iteration: usize,
        rendered_output_sha256: String,
    }

    #[test]
    fn synthetic_validation_rendering_workload_is_stable() {
        let mut db = primed_realistic_db();
        let file = db.file_with_contents("/templates/bench.html", MANY_ERRORS_SOURCE);
        let check = check_file(&db, file);
        assert!(
            !check.validation_errors.is_empty(),
            "synthetic validation error benchmark produced no validation errors",
        );

        let config = DiagnosticsConfig::default();
        let renderer = DiagnosticRenderer::plain();
        let mut rendered_count = 0;
        let mut rendered_bytes = 0;
        let mut hasher = StableHasher::new();
        for error in &check.validation_errors {
            if let Some(output) =
                render_validation_error(MANY_ERRORS_SOURCE, "bench.html", error, &config, &renderer)
            {
                rendered_count += 1;
                rendered_bytes += output.len();
                hasher.write(output.as_bytes());
            }
        }

        insta::assert_yaml_snapshot!(
            "diagnostics_render_validation_synthetic",
            SyntheticValidationRenderSnapshot {
                source_bytes: MANY_ERRORS_SOURCE.len(),
                source_sha256: sha256(MANY_ERRORS_SOURCE.as_bytes()),
                validation_error_count: check.validation_errors.len(),
                rendered_count_per_inner_iteration: rendered_count,
                rendered_bytes_per_inner_iteration: rendered_bytes,
                rendered_output_sha256: hasher.finish(),
            }
        );
    }
}

use djls_source::Diagnostic;
use djls_source::Severity;
use djls_source::Span;

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

fn span_of(source: &'static str, needle: &'static str) -> Result<Span, SyntheticDiagnosticError> {
    let Some(start) = source.find(needle) else {
        return Err(SyntheticDiagnosticError { needle });
    };
    Ok(Span::saturating_from_bounds_usize(
        start,
        start + needle.len(),
    ))
}

#[derive(Clone, Debug, thiserror::Error)]
#[error("synthetic diagnostic source does not contain {needle:?}")]
pub struct SyntheticDiagnosticError {
    needle: &'static str,
}

pub fn synthetic_render_diagnostics() -> Result<[Diagnostic<'static>; 2], SyntheticDiagnosticError>
{
    Ok([
        Diagnostic::new(
            SINGLE_SPAN_SOURCE,
            "templates/page.html",
            "S100",
            "Unclosed tag: block",
            Severity::Error,
            span_of(SINGLE_SPAN_SOURCE, "{% block content %}")?,
            "this block tag is never closed",
        ),
        Diagnostic::new(
            MULTI_SPAN_SOURCE,
            "templates/layout.html",
            "S103",
            "'content' does not match 'sidebar'",
            Severity::Error,
            span_of(MULTI_SPAN_SOURCE, "{% endblock content %}")?,
            "closing tag says 'content'",
        )
        .annotation(
            span_of(MULTI_SPAN_SOURCE, "{% block sidebar %}")?,
            "opening tag is 'sidebar'",
            false,
        ),
    ])
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fmt::Write as _;
    use std::mem::take;
    use std::sync::Arc;
    use std::sync::Mutex;

    use djls::check_template;
    use djls_conf::DiagnosticsConfig;
    use djls_ide::collect_diagnostics;
    use djls_ide::prepare_project_template_analysis;
    use djls_ide::prime_template_library_products;
    use djls_semantic::TemplateDiagnostics;
    use djls_semantic::build_template_tree_for_file;
    use djls_semantic::collect_template_diagnostics;
    use djls_semantic::compute_opaque_regions;
    use djls_semantic::validate_template_file;
    use djls_source::DiagnosticRenderer;
    use djls_source::File;
    use djls_templates::parse_template;
    use insta::assert_yaml_snapshot;
    use salsa::Database as _;
    use salsa::Event;
    use salsa::EventKind;
    use serde::Serialize;
    use sha2::Digest;
    use sha2::Sha256;
    use tower_lsp_server::ls_types::Diagnostic as LspDiagnostic;
    use tower_lsp_server::ls_types::DiagnosticSeverity;
    use tower_lsp_server::ls_types::NumberOrString;

    use super::MANY_ERRORS_SOURCE;
    use super::span_of;
    use super::synthetic_render_diagnostics;
    use crate::CorpusTemplates;
    use crate::DIAGNOSTICS_WARMUP_ITERS;
    use crate::Db;
    use crate::Fixture;
    use crate::bench_corpus_is_required;
    use crate::django_corpus_templates;
    use crate::full_corpus_templates;
    use crate::primed_realistic_db as load_primed_realistic_db;
    use crate::realistic_db as load_realistic_db;
    use crate::structure_db as load_structure_db;
    use crate::template_fixtures as load_template_fixtures;
    use crate::validation_error_fixtures as load_validation_error_fixtures;

    fn template_fixtures() -> &'static [Fixture] {
        load_template_fixtures().expect("template benchmark fixtures should load")
    }

    fn validation_error_fixtures() -> &'static [Fixture] {
        load_validation_error_fixtures().expect("validation error benchmark fixtures should load")
    }

    fn realistic_db() -> Db {
        load_realistic_db().expect("realistic benchmark database should initialize")
    }

    fn primed_realistic_db() -> Db {
        load_primed_realistic_db().expect("primed realistic benchmark database should initialize")
    }

    fn structure_db() -> Db {
        load_structure_db().expect("structure benchmark database should initialize")
    }

    #[test]
    fn synthetic_diagnostic_setup_reports_a_missing_span() {
        let error = span_of("synthetic source", "missing span")
            .expect_err("a missing synthetic diagnostic span should fail setup");

        assert_eq!(
            error.to_string(),
            "synthetic diagnostic source does not contain \"missing span\""
        );
    }

    #[derive(Debug, Eq, PartialEq)]
    struct CheckDigest {
        parser_count: usize,
        validation_count: usize,
        codes: BTreeMap<&'static str, usize>,
    }

    impl CheckDigest {
        fn from_diagnostics(diagnostics: &TemplateDiagnostics) -> Self {
            let mut codes = BTreeMap::new();
            for error in &diagnostics.template_errors {
                *codes.entry(error.diagnostic_code()).or_default() += 1;
            }
            for error in &diagnostics.validation_errors {
                *codes.entry(error.code()).or_default() += 1;
            }

            Self {
                parser_count: diagnostics.template_errors.len(),
                validation_count: diagnostics.validation_errors.len(),
                codes,
            }
        }

        fn total(&self) -> usize {
            self.parser_count + self.validation_count
        }
    }

    #[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
    enum SemanticDiagnosticCategory {
        Parser,
        Validation,
    }

    #[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
    struct NormalizedSpan {
        start: u32,
        length: u32,
    }

    #[derive(Debug, Eq, Ord, PartialEq, PartialOrd)]
    struct NormalizedSemanticDiagnostic {
        category: SemanticDiagnosticCategory,
        code: &'static str,
        primary_span: Option<NormalizedSpan>,
        message: String,
    }

    fn normalize_semantic_diagnostics(
        diagnostics: &TemplateDiagnostics,
    ) -> Vec<NormalizedSemanticDiagnostic> {
        let parser_diagnostics =
            diagnostics
                .template_errors
                .iter()
                .map(|error| NormalizedSemanticDiagnostic {
                    category: SemanticDiagnosticCategory::Parser,
                    code: error.diagnostic_code(),
                    primary_span: error
                        .primary_span()
                        .map(|(start, length)| NormalizedSpan { start, length }),
                    message: error.to_string(),
                });
        let validation_diagnostics =
            diagnostics
                .validation_errors
                .iter()
                .map(|error| NormalizedSemanticDiagnostic {
                    category: SemanticDiagnosticCategory::Validation,
                    code: error.code(),
                    primary_span: error.primary_span().map(|span| NormalizedSpan {
                        start: span.start(),
                        length: span.length(),
                    }),
                    message: error.to_string(),
                });
        let mut normalized = parser_diagnostics
            .chain(validation_diagnostics)
            .collect::<Vec<_>>();
        normalized.sort();
        normalized
    }

    struct StableHasher(Sha256);

    impl StableHasher {
        fn new() -> Self {
            Self(Sha256::new())
        }

        fn write(&mut self, bytes: &[u8]) {
            let length = u64::try_from(bytes.len())
                .expect("benchmark snapshot record length should fit in u64");
            self.0.update(length.to_le_bytes());
            self.0.update(bytes);
        }

        fn finish(self) -> String {
            hex(&self.0.finalize())
        }
    }

    fn hex(bytes: &[u8]) -> String {
        let mut output = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            assert!(
                write!(output, "{byte:02x}").is_ok(),
                "writing a SHA-256 digest to a String should succeed"
            );
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
            let source_length =
                u64::try_from(source.len()).expect("corpus template size should fit in u64");
            hasher.write(&source_length.to_le_bytes());
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

    fn normalize_lsp_diagnostics(
        path: &str,
        diagnostics: Vec<LspDiagnostic>,
    ) -> Vec<NormalizedDiagnostic> {
        let mut normalized = diagnostics
            .into_iter()
            .map(|diagnostic| NormalizedDiagnostic {
                path: path.to_string(),
                code: diagnostic_code(diagnostic.code),
                severity: diagnostic_severity(diagnostic.severity),
                start_line: diagnostic.range.start.line,
                start_character: diagnostic.range.start.character,
                end_line: diagnostic.range.end.line,
                end_character: diagnostic.range.end.character,
                message: diagnostic.message,
            })
            .collect::<Vec<_>>();
        normalized.sort();
        normalized
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
            let parser_count =
                u64::try_from(check.parser_count).expect("parser count should fit in u64");
            hasher.write(&parser_count.to_le_bytes());
            let validation_count =
                u64::try_from(check.validation_count).expect("validation count should fit in u64");
            hasher.write(&validation_count.to_le_bytes());
            for (code, count) in &check.codes {
                hasher.write(code.as_bytes());
                let diagnostic_count =
                    u64::try_from(*count).expect("diagnostic count should fit in u64");
                hasher.write(&diagnostic_count.to_le_bytes());
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
        db: &Db,
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
            let path = file.path(db);
            let diagnostics = collect_template_diagnostics(db, file);
            let digest = CheckDigest::from_diagnostics(&diagnostics);
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

            if let Some(diagnostics) = collect_diagnostics(db, file) {
                ide_eligible_file_count += 1;
                normalized_ide_diagnostics
                    .extend(normalize_lsp_diagnostics(path.as_str(), diagnostics));
            }

            if render {
                let checked =
                    check_template(db, file).expect("benchmark snapshot file should be readable");
                for output in checked.render(&config, &renderer) {
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

    fn take_will_execute_names(db: &Db, events: &Arc<Mutex<Vec<Event>>>) -> Vec<String> {
        let mut events = events
            .lock()
            .expect("benchmark event log lock should not be poisoned");
        take(&mut *events)
            .into_iter()
            .filter_map(|event| match event.kind {
                EventKind::WillExecute { database_key } => Some(
                    db.ingredient_debug_name(database_key.ingredient_index())
                        .to_string(),
                ),
                EventKind::DidValidateMemoizedValue { .. }
                | EventKind::WillBlockOn { .. }
                | EventKind::WillIterateCycle { .. }
                | EventKind::DidFinalizeCycle { .. }
                | EventKind::WillCheckCancellation
                | EventKind::DidSetCancellationFlag
                | EventKind::WillDiscardStaleOutput { .. }
                | EventKind::DidDiscard { .. }
                | EventKind::DidDiscardAccumulated { .. }
                | EventKind::DidInternValue { .. }
                | EventKind::DidReuseInternedValue { .. }
                | EventKind::DidValidateInternedValue { .. } => None,
            })
            .collect()
    }

    fn execution_count(names: &[String], query: &str) -> usize {
        names
            .iter()
            .filter(|name| name.as_str() == query || name.rsplit("::").next() == Some(query))
            .count()
    }

    fn assert_execution_count(names: &[String], query: &str, expected: usize) {
        assert_eq!(
            execution_count(names, query),
            expected,
            "unexpected {query} execution count in {names:#?}"
        );
    }

    #[test]
    fn semantic_incremental_validation_executes_after_each_edit() {
        let fixture = template_fixtures()
            .iter()
            .find(|fixture| fixture.label == "large/views_technical_500.html")
            .expect("benchmark fixtures should include large/views_technical_500.html");
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut db = Db::realistic_with_event_log(Arc::clone(&events));
        let file = db
            .file_with_contents(fixture.path.clone(), &fixture.source)
            .expect("incremental benchmark fixture should register");
        let original = fixture.source.clone();
        let modified = {
            let mut text = original.clone();
            text.push(' ');
            text
        };

        validate_template_file(&db, file);
        let priming_names = take_will_execute_names(&db, &events);
        assert_execution_count(&priming_names, "validate_template_file", 1);

        let original_diagnostics =
            normalize_semantic_diagnostics(&collect_template_diagnostics(&db, file));
        assert!(
            !original_diagnostics.is_empty(),
            "incremental validation fixture should produce semantic diagnostics"
        );
        let original_output_names = take_will_execute_names(&db, &events);
        assert_execution_count(&original_output_names, "validate_template_file", 0);

        db.set_file_contents(file, &modified);
        validate_template_file(&db, file);
        let modified_names = take_will_execute_names(&db, &events);
        assert_execution_count(&modified_names, "validate_template_file", 1);
        let modified_diagnostics =
            normalize_semantic_diagnostics(&collect_template_diagnostics(&db, file));
        let modified_output_names = take_will_execute_names(&db, &events);
        assert_execution_count(&modified_output_names, "validate_template_file", 0);

        db.set_file_contents(file, &original);
        validate_template_file(&db, file);
        let restored_names = take_will_execute_names(&db, &events);
        assert_execution_count(&restored_names, "validate_template_file", 1);
        let restored_diagnostics =
            normalize_semantic_diagnostics(&collect_template_diagnostics(&db, file));
        let restored_output_names = take_will_execute_names(&db, &events);
        assert_execution_count(&restored_output_names, "validate_template_file", 0);

        validate_template_file(&db, file);
        let repeated_names = take_will_execute_names(&db, &events);
        assert_execution_count(&repeated_names, "validate_template_file", 0);

        assert_eq!(modified_diagnostics, original_diagnostics);
        assert_eq!(restored_diagnostics, original_diagnostics);
    }

    #[test]
    fn diagnostics_incremental_collection_executes_after_each_edit() {
        let fixture = template_fixtures()
            .iter()
            .find(|fixture| fixture.label == "large/views_technical_500.html")
            .expect("benchmark fixtures should include large/views_technical_500.html");
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut db = Db::realistic_with_event_log(Arc::clone(&events));
        prime_template_library_products(&db)
            .expect("realistic benchmark database should install a Project before priming");

        let file = db
            .file_with_contents(fixture.path.clone(), &fixture.source)
            .expect("incremental diagnostics benchmark fixture should register");
        let original = fixture.source.clone();
        let modified = {
            let mut text = original.clone();
            text.push(' ');
            text
        };
        let mut original_diagnostics = Vec::new();
        for _ in 0..DIAGNOSTICS_WARMUP_ITERS {
            original_diagnostics = collect_diagnostics(&db, file)
                .expect("incremental Template fixture should be eligible for diagnostics");
        }
        let original_diagnostics =
            normalize_lsp_diagnostics(fixture.path.as_str(), original_diagnostics);
        assert!(
            !original_diagnostics.is_empty(),
            "incremental diagnostics fixture should produce LSP diagnostics"
        );
        drop(take_will_execute_names(&db, &events));

        db.set_file_contents(file, &modified);
        let modified_diagnostics = collect_diagnostics(&db, file)
            .expect("modified Template fixture should be eligible for diagnostics");
        let modified_diagnostics =
            normalize_lsp_diagnostics(fixture.path.as_str(), modified_diagnostics);
        let modified_names = take_will_execute_names(&db, &events);
        assert_execution_count(&modified_names, "validate_template_file", 1);

        db.set_file_contents(file, &original);
        let restored_diagnostics = collect_diagnostics(&db, file)
            .expect("restored Template fixture should be eligible for diagnostics");
        let restored_diagnostics =
            normalize_lsp_diagnostics(fixture.path.as_str(), restored_diagnostics);
        let restored_names = take_will_execute_names(&db, &events);
        assert_execution_count(&restored_names, "validate_template_file", 1);

        let repeated_diagnostics = collect_diagnostics(&db, file)
            .expect("repeated Template fixture should be eligible for diagnostics");
        let repeated_diagnostics =
            normalize_lsp_diagnostics(fixture.path.as_str(), repeated_diagnostics);
        let repeated_names = take_will_execute_names(&db, &events);
        assert_execution_count(&repeated_names, "validate_template_file", 0);

        assert_eq!(modified_diagnostics, original_diagnostics);
        assert_eq!(restored_diagnostics, original_diagnostics);
        assert_eq!(repeated_diagnostics, original_diagnostics);
    }

    #[test]
    fn check_preparation_orders_shared_work_and_kernel_reuses_it() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut db = Db::realistic_with_event_log(Arc::clone(&events));
        let files = template_fixtures()
            .iter()
            .map(|fixture| {
                db.file_with_contents(fixture.path.clone(), &fixture.source)
                    .expect("template benchmark fixture should register")
            })
            .collect::<Vec<_>>();

        prepare_project_template_analysis(&db)
            .expect("check benchmark database should install a Project during preparation");
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

        prepare_project_template_analysis(&db)
            .expect("check benchmark database should retain its Project during preparation");
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
            let checked =
                check_template(&db, *file).expect("check benchmark fixture should be readable");
            drop(checked.render(&config, &renderer));
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
            .map(|fixture| {
                db.file_with_contents(fixture.path.clone(), &fixture.source)
                    .expect("template benchmark fixture should register")
            })
            .collect::<Vec<_>>();
        prepare_project_template_analysis(&db)
            .expect("fixture check benchmark database should install a Project");

        assert_yaml_snapshot!(
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
            .map(|fixture| {
                db.file_with_contents(fixture.path.clone(), &fixture.source)
                    .expect("template benchmark fixture should register")
            })
            .collect::<Vec<_>>();
        let mut snapshots = Vec::new();
        for file in files {
            let nodelist = parse_template(&db, file)
                .expect("semantic benchmark fixture should be a readable Template");
            let tree = build_template_tree_for_file(&db, file, nodelist);
            snapshots.push(SemanticFileSnapshot {
                path: file.path(&db).to_string(),
                template_tree_regions: tree.regions(&db).iter().count(),
                has_opaque_regions: !compute_opaque_regions(&db, file, nodelist).is_empty(),
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

        assert_yaml_snapshot!(
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
                !bench_corpus_is_required(),
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
            .map(|(path, source)| {
                db.file_with_contents(path.clone(), source)
                    .expect("corpus benchmark fixture should register")
            })
            .collect::<Vec<_>>();
        prepare_project_template_analysis(&db)
            .expect("corpus check benchmark database should install a Project");
        assert_yaml_snapshot!(
            name,
            CorpusWorkloadSnapshot {
                input: corpus_input_snapshot(corpus),
                output: checked_workload_snapshot(&db, &files, WorkloadDetail::Corpus, true),
            }
        );
    }

    #[test]
    fn django_corpus_check_workload_is_stable() {
        let corpus = django_corpus_templates().expect("Django corpus templates should load");
        snapshot_corpus("check_workload_corpus_django", corpus);
    }

    #[test]
    fn full_corpus_check_workload_is_stable() {
        let corpus = full_corpus_templates().expect("full corpus templates should load");
        snapshot_corpus("check_workload_corpus_all", corpus);
    }

    #[test]
    fn cached_empty_diagnostics_workload_is_stable() {
        const EMPTY_FILE_COUNT: usize = 6;

        let mut db = primed_realistic_db();
        let files: Vec<_> = (0..EMPTY_FILE_COUNT)
            .map(|index| {
                db.file_with_contents(format!("/templates/empty/{index}.html"), "")
                    .expect("empty benchmark fixture should register")
            })
            .collect();

        assert_yaml_snapshot!(
            "diagnostics_cached_empty",
            checked_workload_snapshot(&db, &files, WorkloadDetail::Small, false)
        );
    }

    #[test]
    fn cached_validation_errors_diagnostics_workload_is_stable() {
        let mut db = primed_realistic_db();
        let files: Vec<_> = validation_error_fixtures()
            .iter()
            .map(|fixture| {
                db.file_with_contents(fixture.path.clone(), &fixture.source)
                    .expect("validation error benchmark fixture should register")
            })
            .collect();

        assert_yaml_snapshot!(
            "diagnostics_cached_validation_errors",
            checked_workload_snapshot(&db, &files, WorkloadDetail::Dense, false)
        );
    }

    #[test]
    fn cached_realistic_diagnostics_workload_is_stable() {
        let mut db = primed_realistic_db();
        let files: Vec<_> = template_fixtures()
            .iter()
            .map(|fixture| {
                db.file_with_contents(fixture.path.clone(), &fixture.source)
                    .expect("template benchmark fixture should register")
            })
            .collect();

        assert_yaml_snapshot!(
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
        let diagnostics = synthetic_render_diagnostics()
            .expect("synthetic rendering diagnostics should be valid");
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

        assert_yaml_snapshot!("diagnostics_render_synthetic", snapshot);
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
            let file = db
                .file_with_contents(fixture.path.clone(), &fixture.source)
                .expect("validation rendering benchmark fixture should register");
            let diagnostics = collect_template_diagnostics(&db, file);
            assert!(
                !diagnostics.validation_errors.is_empty(),
                "validation error rendering fixture '{}' produced no validation errors",
                fixture.label,
            );
            validation_error_count += diagnostics.validation_errors.len();

            let checked = check_template(&db, file)
                .expect("validation rendering benchmark fixture should be readable");
            for output in checked.render(&config, &renderer) {
                rendered_count += 1;
                rendered_bytes += output.len();
                hasher.write(fixture.path.as_str().as_bytes());
                hasher.write(output.as_bytes());
            }
        }

        assert_yaml_snapshot!(
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
        let file = db
            .file_with_contents("bench.html", MANY_ERRORS_SOURCE)
            .expect("synthetic validation benchmark fixture should register");
        let check = collect_template_diagnostics(&db, file);
        assert!(
            !check.validation_errors.is_empty(),
            "synthetic validation error benchmark produced no validation errors",
        );

        let config = DiagnosticsConfig::default();
        let renderer = DiagnosticRenderer::plain();
        let mut rendered_count = 0;
        let mut rendered_bytes = 0;
        let mut hasher = StableHasher::new();
        let checked = check_template(&db, file)
            .expect("synthetic validation benchmark fixture should be readable");
        for output in checked.render(&config, &renderer) {
            rendered_count += 1;
            rendered_bytes += output.len();
            hasher.write(output.as_bytes());
        }

        assert_yaml_snapshot!(
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

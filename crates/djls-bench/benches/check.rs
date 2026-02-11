//! Benchmarks for the `djls check` pipeline.
//!
//! `djls check` is a one-shot CLI command: fresh database, process every
//! file, render diagnostics, exit. There is no warm Salsa cache to
//! benefit from — every benchmark here measures the cold path.
//!
//! Scales from a single file up to the full corpus (~6 000 real-world
//! templates) to stress-test the pipeline end to end.
//!
//! Corpus benchmarks require a synced corpus (`just corpus sync`) and
//! skip gracefully when unavailable.

use std::sync::OnceLock;

use camino::Utf8PathBuf;
use divan::Bencher;
use djls_bench::realistic_db;
use djls_bench::template_fixtures;
use djls_bench::Db;
use djls_bench::TemplateFixture;
use djls_source::DiagnosticRenderer;
use djls_source::Span;

fn main() {
    divan::main();
}

// Replicate the core of `check_file` from the binary crate.
fn run_check(db: &Db, file: djls_source::File) -> CheckResult {
    let source = file.source(db).to_string();
    let path = file.path(db).clone();

    let nodelist = djls_templates::parse_template(db, file);

    let template_errors: Vec<djls_templates::TemplateError> =
        djls_templates::parse_template::accumulated::<djls_templates::TemplateErrorAccumulator>(
            db, file,
        )
        .iter()
        .map(|acc| acc.0.clone())
        .collect();

    let mut validation_errors: Vec<djls_semantic::ValidationError> = Vec::new();

    if let Some(nl) = nodelist {
        djls_semantic::validate_nodelist(db, nl);

        let accumulated = djls_semantic::validate_nodelist::accumulated::<
            djls_semantic::ValidationErrorAccumulator,
        >(db, nl);

        validation_errors = accumulated.iter().map(|acc| acc.0.clone()).collect();
        validation_errors.sort_by_key(|e| e.primary_span().map_or(0, Span::start));
    }

    CheckResult {
        source,
        path,
        template_errors,
        validation_errors,
    }
}

struct CheckResult {
    source: String,
    path: Utf8PathBuf,
    template_errors: Vec<djls_templates::TemplateError>,
    validation_errors: Vec<djls_semantic::ValidationError>,
}

impl CheckResult {
    fn has_diagnostics(&self) -> bool {
        !self.template_errors.is_empty() || !self.validation_errors.is_empty()
    }

    fn render(
        &self,
        config: &djls_conf::DiagnosticsConfig,
        fmt: &DiagnosticRenderer,
    ) -> Vec<String> {
        let mut results = Vec::new();
        let path = self.path.as_str();
        let source = self.source.as_str();

        for error in &self.template_errors {
            if let Some(output) = djls_ide::render_template_error(source, path, error, config, fmt)
            {
                results.push(output);
            }
        }

        for error in &self.validation_errors {
            if let Some(output) =
                djls_ide::render_validation_error(source, path, error, config, fmt)
            {
                results.push(output);
            }
        }

        results
    }
}

// Single file: the unit of work inside `djls check`.
// Each iteration gets a fresh database — no Salsa cache carryover.

#[divan::bench(args = template_fixtures())]
fn check_single_file(fixture: &TemplateFixture) {
    let mut db = realistic_db();
    let file = db.file_with_contents(fixture.path.clone(), &fixture.source);
    let config = djls_conf::DiagnosticsConfig::default();
    let fmt = DiagnosticRenderer::plain();

    let result = run_check(&db, file);
    let rendered = result.render(&config, &fmt);
    divan::black_box(rendered.len());
}

// Batch: all fixture templates through one fresh database.
// The first file pays for TagIndex construction; subsequent files
// reuse it — matching real `djls check` behaviour within a single run.

#[divan::bench]
fn check_batch_fixtures(bencher: Bencher) {
    let fixtures = template_fixtures();

    bencher.bench_local(move || {
        let mut db = realistic_db();
        let config = djls_conf::DiagnosticsConfig::default();
        let fmt = DiagnosticRenderer::plain();

        let files: Vec<_> = fixtures
            .iter()
            .map(|f| db.file_with_contents(f.path.clone(), &f.source))
            .collect();

        let mut total_errors = 0;
        for &file in &files {
            let result = run_check(&db, file);
            total_errors += result.render(&config, &fmt).len();
        }
        divan::black_box(total_errors);
    });
}

// Corpus-scale: real Django templates and the full corpus.

struct CorpusTemplates {
    files: Vec<(Utf8PathBuf, String)>,
}

fn load_corpus_inner(
    get_paths: impl FnOnce(&djls_corpus::Corpus) -> Option<Vec<Utf8PathBuf>>,
) -> Option<CorpusTemplates> {
    let corpus_dir =
        Utf8PathBuf::from(env!("CARGO_WORKSPACE_DIR")).join("crates/djls-corpus/.corpus");
    if !corpus_dir.as_std_path().exists() {
        return None;
    }

    let corpus = djls_corpus::Corpus::require();
    let template_paths = get_paths(&corpus)?;

    let files: Vec<(Utf8PathBuf, String)> = template_paths
        .into_iter()
        .filter_map(|path| {
            let source = std::fs::read_to_string(path.as_std_path()).ok()?;
            Some((path, source))
        })
        .collect();

    if files.is_empty() {
        return None;
    }

    Some(CorpusTemplates { files })
}

fn load_corpus_templates() -> Option<&'static CorpusTemplates> {
    static CORPUS: OnceLock<Option<CorpusTemplates>> = OnceLock::new();
    CORPUS
        .get_or_init(|| {
            load_corpus_inner(|corpus| {
                let django_dir = corpus.latest_package("django")?;
                Some(corpus.templates_in(&django_dir))
            })
        })
        .as_ref()
}

fn load_full_corpus_templates() -> Option<&'static CorpusTemplates> {
    static CORPUS: OnceLock<Option<CorpusTemplates>> = OnceLock::new();
    CORPUS
        .get_or_init(|| load_corpus_inner(|corpus| Some(corpus.templates_in(corpus.root()))))
        .as_ref()
}

// Django's own templates (~123 files). Fresh db each iteration.

#[divan::bench]
fn check_corpus_django(bencher: Bencher) {
    let Some(corpus) = load_corpus_templates() else {
        eprintln!("corpus not synced, skipping");
        return;
    };

    let file_count = corpus.files.len();

    bencher
        .counter(divan::counter::ItemsCount::new(file_count))
        .bench_local(move || {
            let mut db = realistic_db();
            let config = djls_conf::DiagnosticsConfig::default();
            let fmt = DiagnosticRenderer::plain();

            let files: Vec<_> = corpus
                .files
                .iter()
                .map(|(path, source)| db.file_with_contents(path.clone(), source))
                .collect();

            let mut total_errors = 0;
            for &file in &files {
                let result = run_check(&db, file);
                if result.has_diagnostics() {
                    total_errors += result.render(&config, &fmt).len();
                }
            }
            divan::black_box(total_errors);
        });
}

// Full corpus (~6 000 templates from 36 packages). Fresh db each iteration.

#[divan::bench]
fn check_corpus_all(bencher: Bencher) {
    let Some(corpus) = load_full_corpus_templates() else {
        eprintln!("corpus not synced, skipping");
        return;
    };

    let file_count = corpus.files.len();

    bencher
        .counter(divan::counter::ItemsCount::new(file_count))
        .bench_local(move || {
            let mut db = realistic_db();
            let config = djls_conf::DiagnosticsConfig::default();
            let fmt = DiagnosticRenderer::plain();

            let files: Vec<_> = corpus
                .files
                .iter()
                .map(|(path, source)| db.file_with_contents(path.clone(), source))
                .collect();

            let mut total_errors = 0;
            for &file in &files {
                let result = run_check(&db, file);
                if result.has_diagnostics() {
                    total_errors += result.render(&config, &fmt).len();
                }
            }
            divan::black_box(total_errors);
        });
}

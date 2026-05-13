//! Stable benchmark gates for CodSpeed PR comments.
//!
//! Keep small per-fixture and synthetic microbenchmarks in the topic-specific
//! bench targets for local diagnosis. This target intentionally reports fewer,
//! larger signals so PR comments stay useful instead of noisy.

use std::sync::OnceLock;

use camino::Utf8PathBuf;
use divan::Bencher;
use djls_bench::prime;
use djls_bench::python_fixtures;
use djls_bench::realistic_db;
use djls_bench::template_fixtures;
use djls_bench::validation_error_fixtures;
use djls_bench::Db;
use djls_bench::DIAGNOSTICS_INNER_ITERS;
use djls_bench::DIAGNOSTICS_WARMUP_ITERS;
use djls_db::FileCheckResult;
use djls_source::DiagnosticRenderer;

fn main() {
    divan::main();
}

fn run_check(db: &Db, file: djls_source::File) -> FileCheckResult {
    let source = file.source(db);
    let path = file.path(db).clone();
    let check = djls_db::check_file(db, file);

    FileCheckResult {
        source,
        path,
        check,
    }
}

#[divan::bench]
fn gate_parse_templates(bencher: Bencher) {
    let fixtures = template_fixtures();
    bencher.bench_local(move || {
        let mut total_nodes = 0;
        for fixture in fixtures {
            let (nodes, _errors) = djls_templates::parse_template_impl(&fixture.source);
            total_nodes += nodes.len();
        }
        divan::black_box(total_nodes);
    });
}

#[divan::bench]
fn gate_validate_templates_realistic(bencher: Bencher) {
    let fixtures = template_fixtures();

    bencher.bench_local(move || {
        let mut db = realistic_db();
        let files: Vec<_> = fixtures
            .iter()
            .map(|fixture| db.file_with_contents(fixture.path.clone(), &fixture.source))
            .collect();

        for file in &files {
            if let Some(nodelist) = djls_templates::parse_template(&db, *file) {
                djls_semantic::validate_nodelist(&db, nodelist);
            }
        }
    });
}

#[divan::bench]
fn gate_collect_diagnostics_realistic(bencher: Bencher) {
    let fixtures = template_fixtures();
    let mut db = realistic_db();

    let files: Vec<_> = fixtures
        .iter()
        .map(|fixture| {
            let file = db.file_with_contents(fixture.path.clone(), &fixture.source);
            prime(DIAGNOSTICS_WARMUP_ITERS, || {
                let _ = djls_ide::collect_diagnostics(&db, file);
            });
            file
        })
        .collect();

    bencher.bench_local(move || {
        let mut total = 0;
        for _ in 0..DIAGNOSTICS_INNER_ITERS {
            for file in &files {
                total += djls_ide::collect_diagnostics(&db, *file).len();
            }
        }
        divan::black_box(total);
    });
}

struct ValidationRenderFixture<'a> {
    source: &'a str,
    path: &'a str,
    check: djls_db::CheckResult,
}

fn validation_render_fixture(
    fixture: &djls_bench::ValidationErrorFixture,
) -> ValidationRenderFixture<'_> {
    let mut db = realistic_db();
    let file = db.file_with_contents(fixture.path.clone(), &fixture.source);
    let check = djls_db::check_file(&db, file);
    assert!(
        !check.validation_errors.is_empty(),
        "validation error rendering fixture '{}' produced no validation errors",
        fixture.label,
    );

    ValidationRenderFixture {
        source: &fixture.source,
        path: fixture.path.as_str(),
        check,
    }
}

#[divan::bench]
fn gate_render_validation_errors(bencher: Bencher) {
    let fixtures: Vec<_> = validation_error_fixtures()
        .iter()
        .map(validation_render_fixture)
        .collect();
    let config = djls_conf::DiagnosticsConfig::default();
    let renderer = DiagnosticRenderer::plain();

    bencher.bench_local(move || {
        let mut rendered_count = 0;
        for fixture in &fixtures {
            for error in &fixture.check.validation_errors {
                if djls_db::render_validation_error(
                    fixture.source,
                    fixture.path,
                    error,
                    &config,
                    &renderer,
                )
                .is_some()
                {
                    rendered_count += 1;
                }
            }
        }
        divan::black_box(rendered_count);
    });
}

#[divan::bench]
fn gate_check_fixtures_cold(bencher: Bencher) {
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

#[divan::bench]
fn gate_extract_modules(bencher: Bencher) {
    let fixtures = python_fixtures();
    bencher.bench_local(move || {
        for fixture in fixtures {
            divan::black_box(djls_semantic::extract_rules(
                &fixture.source,
                "bench.module",
            ));
        }
    });
}

struct CorpusTemplates {
    files: Vec<(Utf8PathBuf, String)>,
}

fn load_corpus_inner(
    get_paths: impl FnOnce(&djls_corpus::Corpus) -> Option<Vec<Utf8PathBuf>>,
) -> Option<CorpusTemplates> {
    if !djls_corpus::Corpus::is_available() {
        return None;
    }

    let corpus = djls_corpus::Corpus::require();
    let mut template_paths = get_paths(&corpus)?;
    template_paths.sort();

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

fn load_django_corpus_templates() -> Option<&'static CorpusTemplates> {
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

#[divan::bench]
fn gate_check_corpus_django(bencher: Bencher) {
    let Some(corpus) = load_django_corpus_templates() else {
        panic!("corpus not synced; run `just corpus sync` before CodSpeed benchmarks");
    };

    bench_corpus_check(bencher, corpus);
}

fn bench_corpus_check(bencher: Bencher, corpus: &'static CorpusTemplates) {
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

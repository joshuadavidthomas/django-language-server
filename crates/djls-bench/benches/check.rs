//! Benchmarks for the cold validation-and-render kernel used by `djls check`.
//!
//! Divan input setup creates the fixture-backed Project, synchronizes the selected Template
//! sources, then runs production intrinsic priming and Template indexing outside the timed region.
//! The measured kernel validates and renders the synchronized files serially. It intentionally
//! excludes CLI argument/config loading, Django Environment and Project Facts discovery,
//! filesystem Template discovery, Salsa input synchronization, intrinsic priming, Template
//! indexing, Rayon scheduling, sorting, and terminal I/O.
//!
//! The semantic benchmark suite separately measures both a cold Project and a primed
//! Project/cold-Template workload, so excluding setup here does not hide required Project work.
//!
//! Scales from the fixture batch to the full corpus to stress the kernel while
//! keeping that narrower boundary explicit.
//!
//! Corpus benchmarks require a synced corpus (`just corpus sync`) and
//! skip gracefully when unavailable.

use divan::Bencher;
use djls_bench::CorpusLoadError;
use djls_bench::CorpusTemplates;
use djls_bench::Db;
use djls_bench::FileCheckResult;
use djls_bench::django_corpus_templates;
use djls_bench::full_corpus_templates;
use djls_bench::realistic_db;
use djls_bench::template_fixtures;
use djls_source::DiagnosticRenderer;

fn main() {
    divan::main();
}

/// Run `check_file` and capture the source for rendering (mirrors CLI pattern).
fn run_check(db: &Db, file: djls_source::File) -> FileCheckResult {
    let source = file
        .try_source(db)
        .expect("benchmark file should be readable");
    let path = file.path(db).clone();
    let check = djls_bench::check_file(db, file);

    FileCheckResult {
        source,
        path,
        check,
    }
}

// Batch: all fixture templates through one fresh database.
// Per-library semantic products are shared while each Template builds its own
// sparse occurrence projection, matching real `djls check` behaviour.

#[divan::bench]
fn fixtures(bencher: Bencher) {
    let fixtures = template_fixtures();
    bencher
        .with_inputs(move || {
            let mut db = realistic_db();
            let files: Vec<_> = fixtures
                .iter()
                .map(|fixture| db.file_with_contents(fixture.path.clone(), &fixture.source))
                .collect();
            djls_ide::prepare_project_template_analysis(&db)
                .expect("check benchmark database should install a Project");
            (
                db,
                files,
                djls_conf::DiagnosticsConfig::default(),
                DiagnosticRenderer::plain(),
            )
        })
        .bench_local_refs(|(db, files, config, fmt)| {
            let mut total_errors = 0;
            for &file in files.iter() {
                let result = run_check(db, file);
                total_errors += result.render(config, fmt).len();
            }
            divan::black_box(total_errors);
        });
}

// Corpus-scale: real Django templates and the full corpus.

fn bench_corpus_check(
    bencher: Bencher,
    corpus: Result<Option<&'static CorpusTemplates>, CorpusLoadError>,
) {
    let corpus = corpus.unwrap_or_else(|error| panic!("failed to load benchmark corpus: {error}"));
    let Some(corpus) = corpus else {
        assert!(
            std::env::var_os("DJLS_REQUIRE_BENCH_CORPUS").is_none(),
            "corpus not synced; run `just corpus sync` before benchmarks",
        );
        eprintln!("corpus not synced, skipping");
        return;
    };

    let file_count = corpus.files.len();

    bencher
        .counter(divan::counter::ItemsCount::new(file_count))
        .with_inputs(move || {
            let mut db = realistic_db();
            let files: Vec<_> = corpus
                .files
                .iter()
                .map(|(path, source)| db.file_with_contents(path.clone(), source))
                .collect();
            djls_ide::prepare_project_template_analysis(&db)
                .expect("check benchmark database should install a Project");
            (
                db,
                files,
                djls_conf::DiagnosticsConfig::default(),
                DiagnosticRenderer::plain(),
            )
        })
        .bench_local_refs(|(db, files, config, fmt)| {
            let mut total_errors = 0;
            for &file in files.iter() {
                let result = run_check(db, file);
                if result.has_diagnostics() {
                    total_errors += result.render(config, fmt).len();
                }
            }
            divan::black_box(total_errors);
        });
}

// Django's own templates (~123 files). Fresh db each iteration.

#[divan::bench]
fn corpus_django(bencher: Bencher) {
    bench_corpus_check(bencher, django_corpus_templates());
}

// Full corpus (~6 000 templates from 36 packages). Fresh db each iteration.

#[divan::bench]
fn corpus_all(bencher: Bencher) {
    bench_corpus_check(bencher, full_corpus_templates());
}
